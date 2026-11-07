//! O laco de video: quadro codificado -> datagramas QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! Esta parte e **igual** no Linux e no Windows: fatiar, mandar, contar,
//! e encerrar quando ninguem esta mais assistindo. Ficou aqui quando o
//! Windows virou host, e nao copiada para o segundo binario, porque foi
//! justamente aqui que moraram os bugs caros -- o `send_datagram` que
//! falhava calado, a contagem de fps que mentia, a saida que travava no
//! meio da destruicao do NVENC. Duas copias disso divergem, e a copia
//! errada e sempre a que ninguem esta olhando.
//!
//! O que NAO esta aqui e o que de fato difere: de onde vem o quadro
//! (PipeWire x DXGI) e como o input e aplicado (D-Bus x SendInput).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::net::{self, SliceHeader};

/// Quanto esperar por outro cliente antes de encerrar o host.
pub const OCIOSO_S: u64 = 20;

/// Quanto esperar pelo PRIMEIRO cliente antes de desistir.
///
/// O host sobe sob demanda -- o painel o acorda e abre o visualizador em
/// seguida -- entao se ninguem conectou em meio minuto, nao vai mais. Sem
/// este limite, uma tentativa que falhou do lado do cliente deixava a
/// captura e o NVENC rodando na maquina de outra pessoa indefinidamente,
/// porque `accept()` sozinho espera para sempre.
pub const ESPERA_PRIMEIRO_S: u64 = 30;

/// Nanossegundos do relogio de parede.
///
/// De parede e nao monotonico de proposito: o carimbo viaja para a outra
/// maquina, e so da para medir latencia fim-a-fim se os dois lados
/// falarem da mesma origem. Com NTP na LAN o erro e de milissegundos.
pub fn agora_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// As tres bandeiras entre a captura e a rede.
///
/// Juntas numa struct e nao tres `Arc<AtomicBool>` soltos: elas sempre
/// viajam juntas, e tres parametros booleanos seguidos numa assinatura e
/// um convite a passar na ordem errada.
///
/// Bandeira e nao canal porque toda mensagem aqui e idempotente: dois
/// pedidos de quadro-chave valem um, e "saia" dito duas vezes e "saia".
///
/// Tudo em `SeqCst`, e nao `Relaxed`: a ORDEM entre elas importa. A rede
/// liga `idr` antes de `cliente`, e a captura le `cliente` antes de
/// `idr` -- com `Relaxed` ela pode ver "ha cliente" sem ver "manda
/// quadro-chave", e ai o cliente novo recebe um quadro P que nao
/// decodifica. Sao algumas escritas por segundo; a ordem vale mais que
/// os nanossegundos.
#[derive(Default)]
pub struct Sinais {
    /// A rede decidiu encerrar; quem para de verdade e a captura, que e
    /// a dona do NVENC. Matar do outro lado travava no meio da
    /// destruicao dos recursos de CUDA.
    sair: AtomicBool,
    /// O proximo quadro tem de ser IDR.
    idr: AtomicBool,
    /// Ha alguem assistindo.
    cliente: AtomicBool,
    /// Quantos clientes de INPUT estao conectados (o KVM por LAN).
    ///
    /// Separado de `cliente` porque sao papeis diferentes: quem assiste
    /// quer video, quem manda input nao. Sem esta contagem, o laco de
    /// video encerraria o processo por ociosidade enquanto alguem ainda
    /// esta digitando atraves dele -- derrubando o KVM junto.
    entradas: AtomicUsize,
}

impl Sinais {
    pub fn deve_sair(&self) -> bool {
        self.sair.load(Ordering::SeqCst)
    }

    /// Pede que a captura encerre o processo.
    ///
    /// Publico porque quem descobre que nao ha mais o que fazer e a
    /// REDE, e quem pode parar com seguranca e a captura -- ela e dona
    /// do NVENC e do CUDA, e destruir isso do outro lado travava.
    pub fn encerrar(&self) {
        self.sair.store(true, Ordering::SeqCst);
    }

    /// Alguem conectou na porta de input.
    pub fn entrou_input(&self) {
        self.entradas.fetch_add(1, Ordering::SeqCst);
    }

    /// Alguem saiu da porta de input.
    pub fn saiu_input(&self) {
        self.entradas.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn tem_input(&self) -> bool {
        self.entradas.load(Ordering::SeqCst) > 0
    }

    /// Consome o pedido de quadro-chave. Devolve `true` uma vez por
    /// pedido -- quem chama repassa ao codificador.
    pub fn pegar_idr(&self) -> bool {
        self.idr.swap(false, Ordering::SeqCst)
    }

    /// Vale a pena codificar agora?
    ///
    /// Sem ninguem assistindo, codificar e queimar GPU na maquina de
    /// outra pessoa: o quadro iria para um canal que ninguem drena e
    /// seria descartado. Medido no Windows -- 120 quadros descartados
    /// antes de o primeiro cliente conectar.
    pub fn tem_cliente(&self) -> bool {
        self.cliente.load(Ordering::SeqCst)
    }
}

/// Um quadro ja codificado, esperando a rede.
pub struct Quadro {
    pub dados: Vec<u8>,
    pub keyframe: bool,
    pub captured_at_ns: u64,
}

/// Aceita um cliente, transmite enquanto ele estiver lá, e pede parada.
///
/// `abrir_input` recebe a conexao e devolve a tarefa que aplica os
/// eventos. E closure, e nao trait, porque o lado do Linux precisa
/// capturar uma conexao D-Bus que **nao e Send** (roda em `LocalSet`,
/// com `spawn_local`) enquanto o do Windows usa `tokio::spawn` normal --
/// e as duas devolvem o mesmo `JoinHandle`.
pub async fn atender(
    ep: &quinn::Endpoint,
    rx: &mut tokio::sync::mpsc::Receiver<Quadro>,
    sinais: &Sinais,
    mut abrir_input: impl FnMut(&quinn::Connection) -> tokio::task::JoinHandle<()>,
) -> anyhow::Result<()> {
    // UM laco para o primeiro cliente e para os que voltam, com prazos
    // diferentes.
    //
    // Antes eram dois lugares: o laco aceitava o primeiro, e depois de
    // ele sair havia um `accept` separado so para saber se alguem
    // voltava -- que entao encerrava de todo jeito, recusando quem
    // acabara de conectar. O log dizia isso em voz alta ("alguem voltou,
    // mas a conexao nao volta para o laco") e o sintoma era clicar "Ver
    // a tela" duas vezes e a segunda falhar com "o servidor recusou".
    let mut houve_cliente = false;
    loop {
        // Ate o primeiro cliente, o prazo e generoso: o painel acorda o
        // host e abre o visualizador em seguida. Depois dele, o prazo e
        // curto -- quem fechou a janela provavelmente acabou.
        let prazo = if houve_cliente { OCIOSO_S } else { ESPERA_PRIMEIRO_S };
        let entrando = match tokio::time::timeout(Duration::from_secs(prazo), ep.accept()).await {
            Ok(Some(e)) => e,
            // Endpoint fechado: nao ha mais o que aceitar.
            Ok(None) => break,
            Err(_) => {
                // Ociosidade de VIDEO nao e ociosidade do host.
                //
                // Com o KVM conectado na porta de input, ninguem precisa
                // estar assistindo -- a pessoa olha o monitor fisico da
                // outra maquina. Encerrar aqui mataria o processo que
                // esta recebendo as teclas dela, e o teclado voltaria
                // sozinho no meio do uso.
                if sinais.tem_input() {
                    continue;
                }
                if houve_cliente {
                    println!("  ninguem voltou em {prazo}s; encerrando");
                } else {
                    println!("  ninguem conectou em {prazo}s; encerrando");
                }
                sinais.sair.store(true, Ordering::SeqCst);
                return Ok(());
            }
        };
        houve_cliente = true;
        // Handshake que falha NAO e fatal.
        //
        // Com `?` aqui, um unico `timed out` -- cliente morto no meio do
        // aperto de mao, pacote perdido, processo encerrado por SIGTERM --
        // derrubava o laco inteiro. O host seguia VIVO, sem servir
        // ninguem e sem encerrar: as duas tentativas seguintes de
        // conectar nao recebiam nem recusa, so silencio. Medido.
        let conexao = match entrando.await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  handshake falhou ({e}); segue esperando");
                continue;
            }
        };
        println!(
            "  conectado: {} cliente {}",
            conexao.remote_address(),
            crate::identity::describe_peer(&conexao, &crate::identity::authorized(&crate::identity::dir()))
        );
        // O cliente novo nao tem contexto: o proximo quadro TEM de ser
        // quadro-chave, senao a janela dele fica preta ate o IDR
        // seguinte do GOP -- ate 2s olhando nada.
        // Ordem: `idr` ANTES de `cliente`. A captura le `cliente`
        // primeiro, entao quando ela decidir codificar o pedido de
        // quadro-chave ja esta visivel.
        sinais.idr.store(true, Ordering::SeqCst);
        sinais.cliente.store(true, Ordering::SeqCst);

        // Joga fora o que sobrou da sessao anterior.
        //
        // O canal pode ter ate `CAPACIDADE` quadros codificados antes de
        // o cliente de antes sair. Para o cliente NOVO eles sao quadros P
        // sem referencia: o decodificador reclama `non-existing PPS 0
        // referenced` e `no frame!` ate o IDR chegar. Medido reconectando
        // no host do Windows.
        let mut velhos = 0;
        while rx.try_recv().is_ok() {
            velhos += 1;
        }
        if velhos > 0 {
            println!("  descartei {velhos} quadro(s) da sessao anterior");
        }
        let Some(max) = conexao.max_datagram_size() else {
            eprintln!("  ERRO: o par nao aceita datagramas QUIC; nada de video vai passar");
            continue;
        };
        println!("  datagrama maximo: {max} bytes");

        // Input por stream unidirecional: perder um "tecla solta"
        // deixaria a tecla presa no host, entao nao pode ser datagrama.
        let entrada = abrir_input(&conexao);

        let mut quadro: u32 = 0;
        let mut enviados = 0u64;
        // Nada sai antes do primeiro quadro-chave. GARANTIA, nao
        // tentativa.
        //
        // Pedir IDR e drenar o canal cobre o caso comum, mas nao fecha a
        // corrida: a captura le "ha cliente" no topo da volta e so manda
        // o quadro ~20 ms depois, entao um P da sessao anterior pode
        // entrar no canal DEPOIS da drenagem. Medido -- o cliente
        // registrava `recebi quadro 0 P 6342 bytes` seguido de
        // `non-existing PPS 0 referenced`, e o host concordava que havia
        // mandado um P.
        //
        // Descartar aqui nao custa nada: quadro P sem referencia e
        // inutil para quem acabou de chegar, decodificador nenhum
        // aproveita. E a garantia fica LOCAL -- nao depende de ordem de
        // bandeira nem de janela de tempo.
        let mut esperando_chave = true;
        let mut bytes = 0u64;
        let mut recusados = 0u64;
        let t0 = Instant::now();

        loop {
            let c = match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                Ok(Some(c)) => c,
                Ok(None) => break,
                Err(_) => {
                    if conexao.close_reason().is_some() {
                        break;
                    }
                    continue;
                }
            };

            if esperando_chave {
                if !c.keyframe {
                    continue;
                }
                esperando_chave = false;
            }

            // Os primeiros quadros de cada conexao, um a um.
            //
            // A metade que faltava: o cliente dizia "recebi um P antes do
            // quadro-chave" e eu nao tinha como saber se o host mandou
            // errado ou se a rede entregou fora de ordem. Com os dois
            // lados falando, sobra um suspeito.
            if enviados < 3 {
                println!(
                    "  enviando quadro {quadro} {} {} bytes",
                    if c.keyframe { "CHAVE" } else { "P" },
                    c.dados.len()
                );
            }
            let cab = SliceHeader {
                frame: quadro,
                piece: 0,
                pieces: 0,
                keyframe: c.keyframe,
                captured_at_ns: c.captured_at_ns,
            };
            for fatia in net::slice(&c.dados, cab, max) {
                bytes += fatia.len() as u64;
                if let Err(err) = conexao.send_datagram(fatia) {
                    if !matches!(err, quinn::SendDatagramError::TooLarge) {
                        eprintln!("  datagrama recusado: {err}");
                    }
                    recusados += 1;
                    break;
                }
            }
            quadro = quadro.wrapping_add(1);
            enviados += 1;
            if enviados % 120 == 0 {
                let s = t0.elapsed().as_secs_f64();
                println!(
                    "  {enviados} quadros, {:.1} fps, {:.1} Mbps, {recusados} recusado(s)",
                    enviados as f64 / s,
                    bytes as f64 * 8.0 / s / 1_000_000.0
                );
            }
            if conexao.close_reason().is_some() {
                break;
            }
        }
        entrada.abort();
        sinais.cliente.store(false, Ordering::SeqCst);
        println!("  cliente saiu");

        // Volta ao topo do laco: sem ninguem assistindo, capturar e
        // codificar so queima GPU na maquina de outra pessoa, e o
        // `tem_cliente` ja desligou o encoder. O prazo curto decide se
        // vale continuar de pe -- cair o wifi por dois segundos nao
        // deveria exigir voltar ao painel, e fechar a janela de verdade
        // nao deveria deixar processo esquecido.
        println!("  aguardando {OCIOSO_S}s por outro cliente antes de sair");
    }
    // Endpoint fechado por fora. Pedir parada mesmo assim: a captura e
    // quem encerra na ordem certa, e matar do lado da rede travava no
    // meio da destruicao do NVENC.
    sinais.sair.store(true, Ordering::SeqCst);
    Ok(())
}

/// Porta de input, separada da de video.
///
/// `porta + 1`. Existe porque o host serve **um cliente de video por
/// vez**, em sequencia -- e o KVM por LAN competia por essa vaga com o
/// visualizador.
///
/// O sintoma foi exatamente o que o usuario descreveu: clicou para
/// mandar o input para a windows-host, o teclado e o mouse sumiram daqui
/// (o grab pegou), e "no windows parece nao ter mexido". Tinha mexido:
/// o log do host registrou cada `MouseRel`, e medindo o cursor dentro
/// da sessao interativa ele andou 210 px para 200 pedidos. O que nao
/// funcionou foi **ver**: o KVM ocupou a vaga unica e o visualizador
/// nao conseguiu conectar, entao a janela que ele olhava ficou parada.
///
/// Separar as portas resolve dois problemas de uma vez:
///
/// - assistir e controlar deixam de competir;
/// - a maquina alvo para de codificar video para um cliente que nunca
///   vai desenhar nada. Para jogar no Windows com o KVM, isso era GPU
///   queimada em nome de ninguem.
///
/// Varios clientes de input ao mesmo tempo sao permitidos e inofensivos
/// -- os eventos se intercalam, que e o mesmo que acontece com dois
/// teclados fisicos na mesma maquina.
pub async fn atender_input(
    ep: &quinn::Endpoint,
    sinais: std::sync::Arc<Sinais>,
    mut abrir_input: impl FnMut(&quinn::Connection) -> tokio::task::JoinHandle<()>,
) {
    while let Some(entrando) = ep.accept().await {
        if sinais.deve_sair() {
            break;
        }
        let conexao = match entrando.await {
            Ok(c) => c,
            Err(e) => {
                // Handshake ruim nao derruba o laco: o proximo cliente
                // merece a mesma chance. Mesma decisao de `atender`.
                eprintln!("  input: handshake falhou: {e}");
                continue;
            }
        };
        println!(
            "  input conectado: {} cliente {}",
            conexao.remote_address(),
            crate::identity::describe_peer(&conexao, &crate::identity::authorized(&crate::identity::dir()))
        );
        sinais.entrou_input();
        let tarefa = abrir_input(&conexao);

        // Descontar na SAIDA, e por um vigia proprio.
        //
        // Sem isto a contagem so sobe, e `tem_input()` fica verdadeiro
        // para sempre -- o host nunca mais encerraria por ociosidade e
        // ficaria de pe segurando a GPU depois que todo mundo foi
        // embora. O vigia espera a tarefa de leitura terminar, que e
        // exatamente quando o stream fechou.
        // `tokio::spawn` e nao `spawn_local`: o host do Windows roda
        // num current_thread SEM LocalSet, e `spawn_local` entra em
        // panico fora de um. O futuro aqui e Send (um JoinHandle e um
        // Arc de atomicos), entao o spawn comum serve nos dois hosts.
        let s = std::sync::Arc::clone(&sinais);
        tokio::spawn(async move {
            let _ = tarefa.await;
            s.saiu_input();
            println!("  input desconectado");
        });
    }
}
