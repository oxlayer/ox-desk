//! Host no Windows: DXGI -> NVENC -> QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Mais simples que o do Linux, e o motivo importa
//!
//! No Linux a captura e o input passam pelo compositor, e as duas sessoes
//! do Mutter tem de nascer na ordem certa, amarradas por `SessionId`, com
//! a de input pertencendo a conexao D-Bus que a criou. Dai a coreografia
//! entre threads naquele arquivo.
//!
//! Aqui nao ha intermediario: o DXGI entrega a textura e o `SendInput`
//! injeta de qualquer thread. Entao e um laco de captura e uma thread de
//! rede, sem negociacao.
//!
//! ## Precisa da sessao interativa
//!
//! O Desktop Duplication nao existe na sessao 0, e `SendInput` ali nao
//! chega a desktop nenhum. Lancar este processo por SSH NAO funciona --
//! o SSH cai na sessao 0. Quem o acorda e uma tarefa agendada com `/IT`,
//! o mesmo mecanismo que levanta o `logid` (ver `screen::acordar_host`).

#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::capture_win::{Estado, Monitor};
use crate::input::Leitor;
use crate::input_win::Teclado;
use crate::nvenc::Codificador;
use crate::serve::{self, agora_ns, Quadro, Sinais};

/// Dois quadros de folga. Mais que isso seria fila de coisa velha.
const CAPACIDADE: usize = 2;

/// Com a tela parada, reenvia o ultimo quadro a cada tanto.
///
/// Sem isto, um cliente que conecta diante de uma tela imovel ficaria com
/// a janela preta: o DXGI nao entrega quadro quando nada muda, e nosso
/// laco nao teria nada para codificar. A batida tambem sai IDR, entao ela
/// serve de "comeco" para quem acabou de entrar.
const BATIDA: Duration = Duration::from_millis(500);

/// Quanto esperar por quadro novo em cada volta. Um quadro a 60 Hz.
const ESPERA_MS: u32 = 16;

/// Com cliente esperando e NENHUM quadro capturado ainda, de quanto em
/// quanto tempo cutucar a tela para acordar.
///
/// O caso real: a maquina ficou sozinha, a economia de energia desligou
/// a saida de video, e o DXGI passa a devolver `Parado` para sempre --
/// `DuplicateOutput` nem recusa. Sem isto o cliente conectava e ficava
/// na tela preta indefinidamente, e o host ficava de pe sem nada a
/// fazer. Medido: zero quadro em oito segundos com a tela apagada.
const CUTUCADA: Duration = Duration::from_secs(3);

/// Depois disto, a duplicacao e recriada em vez de so cutucar.
///
/// Acordar a tela pode invalidar a duplicacao que nasceu com a saida
/// desligada. Cutucar sozinho nao resolveria esse caso, e recriar e
/// barato perto de deixar o usuario olhando para o nada.
const RECRIAR_APOS: Duration = Duration::from_secs(9);

pub fn executar(porta: u16, monitor: u32) -> anyhow::Result<()> {
    // PRIMEIRA COISA: acordar a tela, e segura-la acordada.
    //
    // Com a tela apagada por economia de energia nao ha saida de video
    // ligada, e `DuplicateOutput` recusa as cinco tentativas -- o host
    // morria dizendo "tela bloqueada", que era diagnostico errado: nao
    // havia senha nenhuma, so o monitor desligado. E justamente quando a
    // maquina ficou sozinha um tempo que voce quer alcanca-la de longe.
    //
    // Antes de QUALQUER coisa de DXGI, inclusive de enumerar a saida:
    // acordar depois de uma chamada que ja falhou nao ajudaria. O guarda
    // vive ate o fim da funcao porque a tela apagando no meio da sessao
    // derrubaria a duplicacao com ACCESS_LOST -- nao basta acordar na
    // partida. Sai no `Drop`, entao nao prendemos a maquina acordada
    // depois que o host morre.
    //
    // NAO resolve sessao realmente bloqueada: ali o desktop seguro esta
    // na frente e o DXGI nao o captura por desenho. Limite conhecido.
    let _acesa = crate::input_win::TelaAcesa::nova();
    crate::input_win::acordar_tela();
    // E o protetor de tela, que e problema DIFERENTE da tela apagada:
    // ele roda num desktop proprio e o DXGI recusa duplicar para quem
    // nao esta no desktop de input. `acordar_tela()` nao o dispensa,
    // porque `SendInput` vai para o desktop da thread que chama.
    crate::input_win::encerrar_protetor_de_tela();

    // ORDEM IMPORTA: o encoder primeiro, a duplicacao depois.
    //
    // Segurar a duplicacao do DXGI enquanto o CUDA inicializa travava o
    // processo -- parava em "DXGI: monitor 0" com 0,2 s de CPU e nunca
    // mais saia de la, enquanto a sonda de NVENC sozinha rodava a 111 fps
    // na mesma sessao. Abrir o monitor (barato) da o tamanho sem comecar
    // a capturar; a captura comeca quando ja ha para onde mandar.
    let mon = Monitor::abrir(monitor)?;
    let (w, h) = mon.tamanho();
    // Compartilhada com a thread de rede: o input converte pixel do
    // stream em posicao do desktop virtual, e isso depende de onde este
    // monitor esta.
    let origem = Arc::new(Mutex::new(mon.origem()));
    let mut cod = Codificador::novo(w, h)?;
    println!("  NVENC pronto para {w}x{h}, {:.1} Mbps alvo", cod.alvo_mbps());
    let mut tela = mon.duplicar()?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Quadro>(CAPACIDADE);
    let sinais = Arc::new(Sinais::default());

    let rede = {
        let sinais = Arc::clone(&sinais);
        let origem = Arc::clone(&origem);
        std::thread::spawn(move || {
            if let Err(e) = servir(porta, rx, Arc::clone(&sinais), origem) {
                eprintln!("  rede: {e}");
            }
            // A rede acabou -- por sucesso ou por erro. Em qualquer caso
            // nao ha mais para quem mandar quadro, e quem encerra na
            // ordem certa e a captura. Sem esta linha, um erro na rede
            // deixava o processo vivo para sempre, capturando para
            // ninguem.
            sinais.encerrar();
        })
    };

    println!("  host na porta {porta}; ctrl-c para parar");
    let mut ultimo_envio = Instant::now();
    let mut descartados = 0u64;
    // Vigia de tela muda: desde quando ha cliente esperando sem que a
    // captura tenha produzido um unico quadro.
    let mut mudo_desde: Option<Instant> = None;
    let mut ultima_cutucada: Option<Instant> = None;
    let mut ja_recriou = false;

    while !sinais.deve_sair() {
        // Sem ninguem assistindo, nao codifica: o quadro iria para um
        // canal que ninguem drena. O `AcquireNextFrame` continua sendo
        // chamado porque e ele que mantem a duplicacao viva e detecta
        // perda de acesso -- so o NVENC e que fica parado.
        let assistindo = sinais.tem_cliente();
        if sinais.pegar_idr() {
            cod.pedir_idr();
        }

        // Vigia de tela muda.
        //
        // `tem_imagem()` falso com cliente conectado significa que nunca
        // capturamos nada para mandar -- a batida de tela parada nao tem
        // o que reenviar e o laco faria `continue` para sempre. Era
        // exatamente isso que acontecia com a tela apagada: o cliente
        // ficava no preto e o host nao reclamava.
        if assistindo && !tela.tem_imagem() {
            // Dois relogios: o total decide quando recriar, o da
            // cutucada decide quando cutucar de novo. Com um so, reiniciar
            // o relogio para espacar as cutucadas apagaria o total e o
            // `recriar` nunca chegaria.
            let desde = *mudo_desde.get_or_insert_with(Instant::now);
            let mudo_ha = desde.elapsed();
            let cutucar = ultima_cutucada.map_or(mudo_ha >= CUTUCADA, |t: Instant| {
                t.elapsed() >= CUTUCADA
            });

            if mudo_ha >= RECRIAR_APOS && !ja_recriou {
                ja_recriou = true;
                println!("  nenhum quadro em {}s; recriando a duplicacao", mudo_ha.as_secs());
                if let Err(e) = tela.recriar() {
                    eprintln!("  recriar apos tela muda: {e}");
                }
            } else if cutucar {
                println!("  nenhum quadro em {}s; acordando a tela", mudo_ha.as_secs());
                crate::input_win::acordar_tela();
                ultima_cutucada = Some(Instant::now());
            }
        } else if tela.tem_imagem() {
            mudo_desde = None;
            ultima_cutucada = None;
            ja_recriou = false;
        }

        let estado = match tela.proximo(ESPERA_MS) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("  captura: {e}");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };

        match estado {
            Estado::Perdido => {
                // Tela de UAC, mudanca de resolucao, jogo assumindo o
                // display. Nao e erro: e recriar e seguir. Falhar aqui e
                // normal enquanto o desktop seguro esta na frente, por
                // isso a pausa antes de tentar de novo.
                println!("  captura perdida; recriando");
                match tela.recriar() {
                    Ok(mudou) => {
                        if let Ok(mut g) = origem.lock() {
                            *g = tela.origem();
                        }
                        if mudou {
                            let (w, h) = tela.tamanho();
                            cod = Codificador::novo(w, h)?;
                            println!("  NVENC refeito para {w}x{h}");
                        }
                    }
                    Err(e) => {
                        eprintln!("  recriar: {e}");
                        std::thread::sleep(Duration::from_millis(300));
                    }
                }
                continue;
            }
            Estado::Parado => {
                if !assistindo || !tela.tem_imagem() || ultimo_envio.elapsed() < BATIDA {
                    continue;
                }
                // Batida de tela parada: sai IDR para servir de comeco a
                // quem conectou agora.
                cod.pedir_idr();
            }
            Estado::Novo => {
                if !assistindo {
                    continue;
                }
            }
        }

        let capturado = agora_ns();
        let codificado = {
            let m = match tela.mapear() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("  mapear: {e}");
                    continue;
                }
            };
            cod.codificar(m.px, m.stride)
        };
        match codificado {
            Ok(dados) => {
                let keyframe = cod.ultimo_foi_idr();
                ultimo_envio = Instant::now();
                // `try_send`: se a rede nao acompanha, o quadro novo e
                // descartado AQUI em vez de virar fila. Quadro atrasado
                // nao tem valor numa tela remota.
                if tx.try_send(Quadro { dados, keyframe, captured_at_ns: capturado }).is_err() {
                    descartados += 1;
                    if descartados % 120 == 0 {
                        println!("  {descartados} quadros descartados (rede mais lenta que a GPU)");
                    }
                }
            }
            Err(e) => eprintln!("  {e}"),
        }
    }

    println!("  captura encerrada");
    let _ = rede.join();
    Ok(())
}

/// Rede: serve video por datagrama e recebe input por stream.
///
/// Diferente do Linux, aqui o `tokio::spawn` normal serve: o `SendInput`
/// nao tem dono de thread nem sessao de D-Bus para respeitar.
fn servir(
    porta: u16,
    mut rx: tokio::sync::mpsc::Receiver<Quadro>,
    sinais: Arc<Sinais>,
    origem: Arc<Mutex<(i32, i32)>>,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let escuta = crate::bind::escuta();
        let ep = crate::net::servidor((escuta, porta).into())?;
        // O endereco REAL, nao "0.0.0.0" fixo: com --bind overlay o
        // host escuta em outro lugar, e um log que diz 0.0.0.0
        // mandaria quem diagnostica procurar no lugar errado.
        println!("  escutando em {escuta}:{porta}");

        // Porta de input, ao lado da de video.
        //
        // O host serve um cliente de VIDEO por vez, e o KVM por LAN
        // disputava essa vaga com o visualizador. Em portas separadas,
        // assistir e controlar deixam de competir -- e a maquina para
        // de codificar video para um cliente que nunca desenha nada.
        let porta_input = porta + 1;
        let ep_in = crate::net::servidor((escuta, porta_input).into())?;
        println!("  input em {escuta}:{porta_input}");
        {
            let sinais_in = Arc::clone(&sinais);
            let origem_in = Arc::clone(&origem);
            tokio::spawn(async move {
                serve::atender_input(&ep_in, Arc::clone(&sinais_in), |conexao| {
                    let conexao = conexao.clone();
                    let origem = Arc::clone(&origem_in);
                    tokio::spawn(async move { receber_input(conexao, origem).await })
                })
                .await;
            });
        }

        serve::atender(&ep, &mut rx, &sinais, |conexao| {
            let conexao = conexao.clone();
            let origem = Arc::clone(&origem);
            tokio::spawn(async move { receber_input(conexao, origem).await })
        })
        .await
    })
}

/// Le o stream de input de uma conexao e injeta ate ela fechar.
///
/// Um so para as duas portas: o visualizador manda input pela conexao
/// de video, o KVM manda pela porta de input, e os dois merecem o
/// mesmo tratamento. Duplicar isto seria convidar as duas copias a
/// divergirem -- e uma delas e a que o usuario usa para digitar.
async fn receber_input(conexao: quinn::Connection, origem: Arc<Mutex<(i32, i32)>>) {
    let Ok(mut recv) = conexao.accept_uni().await else { return };
    let mut teclado = Teclado::novo(origem);
    let mut leitor = Leitor::default();
    let mut buf = vec![0u8; 4096];
    let mut n = 0u64;
    while let Ok(Some(lidos)) = recv.read(&mut buf).await {
        leitor.alimentar(&buf[..lidos]);
        while let Some(e) = leitor.proximo() {
            teclado.aplicar(e);
            n += 1;
            // Os primeiros, um a um: "nao deu erro" e prova fraca de
            // que o input chegou. Depois so o agregado, para nao
            // encher o log a 60 Hz.
            if n <= 8 {
                println!("  input #{n}: {e:?}");
            } else if n % 500 == 0 {
                println!("  {n} eventos de input aplicados");
            }
        }
    }
}
