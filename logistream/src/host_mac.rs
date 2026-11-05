//! Host no macOS: avfoundation -> VideoToolbox -> QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que pelo ffmpeg, e nao por ScreenCaptureKit cru
//!
//! Medido na maquina (ver `logistream-probe-mac`): o ffmpeg do brew tem
//! `avfoundation` como dispositivo de entrada e `h264_videotoolbox` como
//! encoder por hardware. Os dois juntos dao o pipeline inteiro sem FFI
//! novo -- e a biblioteca ja esta instalada porque e ela que decodifica
//! no visualizador de la.
//!
//! ScreenCaptureKit e VideoToolbox crus seriam centenas de linhas de
//! ponte com Objective-C para chegar no mesmo lugar, com a unica
//! vantagem de dispensar o brew. Nao vale.
//!
//! ## Diferenca visivel em relacao aos outros dois hosts
//!
//! O `avfoundation` sabe desenhar o cursor (`capture_cursor`), igual ao
//! Mutter e ao contrario do DXGI. Entao a tela do Mac vai com cursor
//! embutido, como a do Linux.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::serve::{self, agora_ns, Quadro, Sinais};

/// Dois quadros de folga. Mais que isso seria fila de coisa velha.
const CAPACIDADE: usize = 2;

/// Cadencia alvo: 60 Hz, o mesmo teto do host do Linux e pela mesma
/// razao -- acima da taxa de atualizacao de quem assiste nao vira
/// imagem, vira banda e trabalho de conversao de cor.
const CADENCIA: Duration = Duration::from_micros(16_667);

/// Abre a captura de tela do `avfoundation`.
///
/// O indice vem como texto porque e isso que a URL do dispositivo e: no
/// avfoundation a tela 0 e `"1"` quando ha uma camera na frente dela, e
/// o unico jeito honesto de saber e listar -- entao o indice e
/// configuravel e o default e o que funciona nesta maquina.
fn abrir(indice: &str, fps: u32) -> anyhow::Result<ffmpeg_next::format::context::Input> {
    let nome = std::ffi::CString::new("avfoundation")?;
    // `format::list()` existia e foi removido no ffmpeg 5. Procurar pelo
    // nome e o caminho atual, e e o que o proprio `ffmpeg -f` faz.
    let raw = unsafe { ffmpeg_next::ffi::av_find_input_format(nome.as_ptr()) };
    if raw.is_null() {
        anyhow::bail!("avfoundation nao esta neste ffmpeg");
    }
    let entrada = unsafe { ffmpeg_next::format::format::Input::wrap(raw as *mut _) };
    let fmt = ffmpeg_next::format::format::Format::Input(entrada);

    let mut opts = ffmpeg_next::Dictionary::new();
    opts.set("framerate", &fps.to_string());
    // Cursor na imagem: sem isto quem assiste nao ve para onde aponta.
    // O DXGI nao oferece essa opcao; aqui oferece, entao usamos.
    opts.set("capture_cursor", "1");
    // Sem clique desenhado: circulo branco a cada clique e distracao.
    opts.set("capture_mouse_clicks", "0");
    // SEM `pixel_format`: a captura de tela do avfoundation recusa a
    // opcao e cai no default avisando `Configuration of video device
    // failed, falling back to default` -- medido. O formato que ela
    // entregar e convertido depois, pelo `swscale`, que ja esta no
    // caminho de qualquer jeito.

    let ctx = ffmpeg_next::format::open_with(&indice, &fmt, opts)
        .map_err(|e| anyhow::anyhow!("abrir tela {indice}: {e}"))?;
    match ctx {
        ffmpeg_next::format::context::Context::Input(i) => Ok(i),
        _ => anyhow::bail!("avfoundation devolveu contexto de saida"),
    }
}

pub fn executar(porta: u16, indice: &str) -> anyhow::Result<()> {
    ffmpeg_next::init()?;
    // Sem isto o nome `avfoundation` nao resolve: os dispositivos de
    // entrada vivem no libavdevice e precisam ser registrados.
    ffmpeg_next::device::register_all();

    // A captura trava, nao falha, quando falta a permissao de
    // Gravacao de Tela -- e o processo fica em `S` sem um unico erro.
    // Avisar ANTES de tentar: se ficarmos pendurados, pelo menos a
    // ultima linha do log diz onde procurar.
    println!("  abrindo a tela {indice}... (se parar aqui, falta Gravacao de Tela para este binario)");
    let mut entrada = abrir(indice, 60)?;
    let fluxo = entrada
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("a captura nao tem fluxo de video"))?;
    let indice_fluxo = fluxo.index();
    let mut dec = ffmpeg_next::codec::context::Context::from_parameters(fluxo.parameters())?
        .decoder()
        .video()?;
    let (w, h) = (dec.width(), dec.height());
    println!("  avfoundation: tela {indice}, {w}x{h}, {:?}", dec.format());

    let codec = ffmpeg_next::encoder::find_by_name("h264_videotoolbox")
        .ok_or_else(|| anyhow::anyhow!("sem h264_videotoolbox"))?;
    let mut enc = ffmpeg_next::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    enc.set_width(w);
    enc.set_height(h);
    // NV12 e o formato nativo do VideoToolbox; pedir outro faria ele
    // converter por dentro, sem nos avisar do custo.
    enc.set_format(ffmpeg_next::format::Pixel::NV12);
    enc.set_time_base(ffmpeg_next::Rational::new(1, 60));
    enc.set_frame_rate(Some(ffmpeg_next::Rational::new(60, 1)));
    // Mesma escala dos outros dois hosts (ver `nvenc::Codificador::novo`):
    // 4 bits por pixel por segundo a 60 fps.
    let alvo = ((w as u64) * (h as u64) * 4).clamp(8_000_000, 60_000_000);
    enc.set_bit_rate(alvo as usize);
    enc.set_max_bit_rate((alvo * 2) as usize);
    // IDR a cada 2s: perdeu quadro, o erro cicatriza em 2s em vez de
    // sujar a imagem para sempre.
    enc.set_gop(120);
    // Sem quadros B: melhoram compressao a custo de atrasar a saida, e
    // aqui latencia vale mais que bitrate.
    enc.set_max_b_frames(0);
    let mut opts = ffmpeg_next::Dictionary::new();
    // `realtime`: o VideoToolbox deixa de acumular quadros para decidir
    // melhor, que e o oposto do que tela remota quer.
    opts.set("realtime", "true");
    let mut enc = enc.open_with(opts)?;
    println!("  h264_videotoolbox pronto, {} Mbps alvo", alvo / 1_000_000);

    // Conversor para NV12, criado so se o formato da captura diferir.
    //
    // Nao da para pedir NV12 na captura (a opcao e recusada), e alimentar
    // o VideoToolbox com outro formato faria ele converter por dentro,
    // sem nos contar o custo. Convertendo aqui, o custo e visivel e fica
    // num lugar que da para medir.
    let mut escalador = if dec.format() == ffmpeg_next::format::Pixel::NV12 {
        None
    } else {
        println!("  convertendo {:?} -> NV12", dec.format());
        Some(ffmpeg_next::software::scaling::Context::get(
            dec.format(),
            w,
            h,
            ffmpeg_next::format::Pixel::NV12,
            w,
            h,
            ffmpeg_next::software::scaling::Flags::FAST_BILINEAR,
        )?)
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Quadro>(CAPACIDADE);
    let sinais = Arc::new(Sinais::default());
    let origem = Arc::new(Mutex::new((0i32, 0i32)));

    let rede = {
        let sinais = Arc::clone(&sinais);
        let origem = Arc::clone(&origem);
        std::thread::spawn(move || {
            if let Err(e) = servir(porta, rx, Arc::clone(&sinais), origem) {
                eprintln!("  rede: {e}");
            }
            sinais.encerrar();
        })
    };

    println!("  host na porta {porta}; ctrl-c para parar");
    // Avisar AGORA, nao quando o input nao funcionar.
    //
    // Sem permissao de Acessibilidade o `CGEventPost` aceita o evento e
    // o descarta em silencio -- devolve sucesso e nada acontece. E o
    // mesmo modo de falha da Gravacao de Tela, e foi ele que fez o
    // usuario passar uma tarde achando que o KVM estava quebrado.
    if crate::input_mac::tem_acessibilidade() {
        println!("  acessibilidade concedida: teclado e mouse serao injetados");
        // Dito no log porque "a posicao do cursor nao bateu" e um
        // sintoma que nao da para diagnosticar sem este numero: ele
        // diz em que espaco o ponteiro e injetado contra o espaco em
        // que a tela e capturada.
        let e = crate::input_mac::escala_do_display();
        println!("  {e:.0} pixel(s) de captura por ponto de ponteiro");
    } else {
        println!(
            "  AVISO: SEM permissao de Acessibilidade. O teclado e o mouse que \
             chegarem serao DESCARTADOS em silencio pelo macOS. Conceda em \
             Ajustes > Privacidade e Seguranca > Acessibilidade, para \
             logistream-host."
        );
    }
    let mut proximo = Instant::now();
    let mut n = 0i64;
    let mut descartados = 0u64;

    // `Packet::read` direto, e NAO o iterador `packets()`.
    //
    // O iterador encerra em qualquer erro que nao seja fim de arquivo --
    // inclusive `EAGAIN`, que e a resposta normal de um dispositivo de
    // captura que ainda nao tem quadro pronto. A documentacao da propria
    // crate diz isso: "callers that must observe these errors drive
    // `Packet::read` directly".
    //
    // O sintoma foi exato: o host abria a tela, montava o encoder,
    // anunciava a porta e imprimia "captura encerrada" **antes** de
    // alguem conectar. O log do usuario trazia a pista uma linha acima:
    // `not enough frames to estimate rate`. O dispositivo tinha acabado
    // de abrir e nao tinha quadro; a primeira leitura devolveu "tente de
    // novo"; o iterador leu isso como fim do fluxo.
    let mut pacote = ffmpeg_next::Packet::empty();
    loop {
        if sinais.deve_sair() {
            break;
        }
        match pacote.read(&mut entrada) {
            Ok(()) => {}
            // Fim de verdade: o dispositivo sumiu.
            Err(ffmpeg_next::Error::Eof) => break,
            // Qualquer outra coisa e "ainda nao": captura nao termina, ela
            // espera. Dormir um pouco evita girar a 100% de CPU.
            Err(_) => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
        }
        if pacote.stream() != indice_fluxo {
            continue;
        }
        if dec.send_packet(&pacote).is_err() {
            continue;
        }
        let mut quadro = ffmpeg_next::frame::Video::empty();
        while dec.receive_frame(&mut quadro).is_ok() {
            // Sem ninguem assistindo, nao codifica: o quadro iria para um
            // canal que ninguem drena.
            if !sinais.tem_cliente() {
                continue;
            }
            let agora = Instant::now();
            if agora < proximo {
                continue;
            }
            proximo = (proximo + CADENCIA).max(agora);

            // Converte quando preciso. `nv12` e o quadro que vai ao
            // encoder: o original quando ja esta certo, o convertido
            // quando nao.
            let mut convertido = ffmpeg_next::frame::Video::empty();
            let quadro = match escalador.as_mut() {
                Some(sc) => {
                    if sc.run(&quadro, &mut convertido).is_err() {
                        continue;
                    }
                    &mut convertido
                }
                None => &mut quadro,
            };

            let capturado = agora_ns();
            if sinais.pegar_idr() {
                // Pedido de quadro-chave: o cliente novo nao tem contexto.
                quadro.set_kind(ffmpeg_next::picture::Type::I);
            } else {
                quadro.set_kind(ffmpeg_next::picture::Type::None);
            }
            quadro.set_pts(Some(n));
            n += 1;
            if enc.send_frame(&quadro).is_err() {
                continue;
            }
            let mut saida = ffmpeg_next::Packet::empty();
            while enc.receive_packet(&mut saida).is_ok() {
                let Some(dados) = saida.data() else { continue };
                let keyframe = saida.is_key();
                if tx
                    .try_send(Quadro {
                        dados: dados.to_vec(),
                        keyframe,
                        captured_at_ns: capturado,
                    })
                    .is_err()
                {
                    descartados += 1;
                    if descartados % 120 == 0 {
                        println!("  {descartados} quadros descartados (rede mais lenta que a GPU)");
                    }
                }
            }
        }
    }

    println!("  captura encerrada");
    let _ = rede.join();
    Ok(())
}

/// Rede: video por datagrama, input por stream.
///
/// `tokio::spawn` normal, como no Windows: o `CGEvent` nao tem dono de
/// thread. Diferente do Linux, onde a sessao do Mutter pertence a
/// conexao D-Bus que a criou.
fn servir(
    porta: u16,
    mut rx: tokio::sync::mpsc::Receiver<Quadro>,
    sinais: Arc<Sinais>,
    _origem: Arc<Mutex<(i32, i32)>>,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let ep = crate::net::servidor(([0, 0, 0, 0], porta).into())?;
        println!("  escutando em 0.0.0.0:{porta}");

        // Porta de input, ao lado da de video.
        //
        // Faltava aqui: o host do Windows e o do Linux ja tinham, e o
        // do Mac nao -- entao o KVM nao achava onde mandar e dava
        // timeout. Sem ela, assistir e controlar disputam a vaga unica
        // de cliente de video.
        let porta_input = porta + 1;
        let ep_in = crate::net::servidor(([0, 0, 0, 0], porta_input).into())?;
        println!("  input em 0.0.0.0:{porta_input}");
        {
            let sinais_in = Arc::clone(&sinais);
            tokio::spawn(async move {
                serve::atender_input(&ep_in, Arc::clone(&sinais_in), |conexao| {
                    let conexao = conexao.clone();
                    tokio::spawn(async move { receber_input(conexao).await })
                })
                .await;
            });
        }

        serve::atender(&ep, &mut rx, &sinais, |conexao| {
            let conexao = conexao.clone();
            tokio::spawn(async move { receber_input(conexao).await })
        })
        .await
    })
}

/// Le o stream de input de uma conexao e injeta ate ela fechar.
///
/// Um so para as duas portas: o visualizador manda input pela conexao
/// de video, o KVM manda pela porta de input, e os dois merecem o
/// mesmo tratamento.
async fn receber_input(conexao: quinn::Connection) {
    let Ok(mut recv) = conexao.accept_uni().await else { return };
    // Canal por conexao: a thread morre quando o cliente sai, e um
    // `Teclado` novo comeca com a posicao real do ponteiro.
    let teclado = crate::input_mac::thread();
    let mut leitor = crate::input::Leitor::default();
    let mut buf = vec![0u8; 4096];
    let mut n = 0u64;
    while let Ok(Some(lidos)) = recv.read(&mut buf).await {
        leitor.alimentar(&buf[..lidos]);
        while let Some(e) = leitor.proximo() {
            if teclado.send(e).is_err() {
                return; // a thread de injecao saiu
            }
            n += 1;
            // Os primeiros, um a um: "nao deu erro" e prova fraca de
            // que o input chegou -- e no macOS e prova NENHUMA, porque
            // sem Acessibilidade o CGEventPost devolve sucesso e
            // descarta.
            if n <= 8 {
                println!("  input #{n}: {e:?}");
            } else if n % 500 == 0 {
                println!("  {n} eventos de input aplicados");
            }
        }
    }
}
