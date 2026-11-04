//! Host: tela -> NVENC -> QUIC. A v2 do lado de ca.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Por que duas threads e um canal
//!
//! O laco do PipeWire e sincrono e dono da sua thread; o `quinn` e
//! async. Chamar rede de dentro do callback de quadro misturaria os dois
//! mundos e, pior, qualquer hesitacao da rede viraria quadro perdido na
//! captura. O canal desacopla: o callback codifica e **larga**; a thread
//! de rede manda.
//!
//! O canal tem fundo raso de proposito (`CAPACIDADE`). Se a rede nao
//! acompanha, o certo e **descartar quadro velho**, nao enfileirar: numa
//! tela remota, quadro atrasado nao tem valor nenhum -- ja passou.
//!
//!   cargo run --release --bin host -- [porta] [conector]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cudarc::driver::CudaContext;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use dbus::Path;
use logistream::input::{Evento, Leitor};
use logistream::net::{self, SliceHeader};
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PARAMS_RC_MODE, NV_ENC_PRESET_P4_GUID, NV_ENC_TUNING_INFO,
};
use moq_nvenc::{EncodePictureParams, Encoder, EncoderInitParams};
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::video::VideoFormat;
use pipewire::spa::pod::{self, Pod};
use pipewire::spa::utils::{Direction, SpaTypes};
use pipewire::stream::{Stream, StreamFlags};

const SC: &str = "org.gnome.Mutter.ScreenCast";
const SC_OBJ: &str = "/org/gnome/Mutter/ScreenCast";
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";
const STREAM: &str = "org.gnome.Mutter.ScreenCast.Stream";
const RD: &str = "org.gnome.Mutter.RemoteDesktop";
const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
const RD_SESSION: &str = "org.gnome.Mutter.RemoteDesktop.Session";
const T: Duration = Duration::from_secs(10);
/// Dois quadros de folga. Mais que isso seria fila de coisa velha.
const CAPACIDADE: usize = 2;
/// Quanto esperar por outro cliente antes de encerrar.
const OCIOSO_S: u64 = 20;

struct Codificado {
    dados: Vec<u8>,
    keyframe: bool,
    captured_at_ns: u64,
}

fn agora_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let porta: u16 = a.first().and_then(|v| v.parse().ok()).unwrap_or(9777);
    let conector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());

    // Canal do TOKIO, nao `std::sync::mpsc`.
    //
    // Com o canal da std, o laco de rede bloqueava a thread em
    // `recv_timeout` -- e numa runtime `current_thread` isso impede o
    // driver da conexao de rodar. `send_datagram` so ENFILEIRA; quem
    // poe no fio e o driver. Resultado medido: host anunciando 80 fps e
    // "0 recusados" com o receptor em zero, porque nada era transmitido.
    let (tx, rx) = tokio::sync::mpsc::channel::<Codificado>(CAPACIDADE);

    // --- coreografia entre as duas threads -------------------------------
    //
    // A sessao de INPUT tem de existir antes da de TELA: e o `SessionId`
    // dela que amarra as duas, e sem o vinculo o
    // `NotifyPointerMotionAbsolute` nao tem espaco de coordenadas.
    //
    // Mas quem injeta e a thread de rede, e no D-Bus a sessao pertence a
    // conexao que a criou -- outra conexao nao pode chamar `Notify*`
    // nela. Entao a sessao de input nasce LA, e so o identificador
    // atravessa.
    //
    //   rede:  cria input  -> manda SessionId ----> main
    //   main:  cria tela vinculada, RecordMonitor -> manda caminho do
    //          stream (para o ponteiro absoluto) -> rede
    //   rede:  Start() -- que inicia as duas
    //   main:  recebe PipeWireStreamAdded e segue
    let (tx_id, rx_id) = std::sync::mpsc::channel::<String>();
    let (tx_pronto, rx_pronto) = std::sync::mpsc::channel::<String>();

    let rede = std::thread::spawn(move || {
        if let Err(e) = servir(porta, rx, tx_id, rx_pronto) {
            eprintln!("  rede: {e}");
        }
    });

    let rd_id = rx_id
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| anyhow::anyhow!("a thread de rede nao criou a sessao de input"))?;

    // --- Mutter ---------------------------------------------------------
    let conn = Connection::new_session()?;
    let root = conn.with_proxy(SC, SC_OBJ, T);
    let mut sc_props = dbus::arg::PropMap::new();
    sc_props.insert(
        "remote-desktop-session-id".into(),
        dbus::arg::Variant(Box::new(rd_id) as Box<dyn dbus::arg::RefArg>),
    );
    let (sess_path,): (Path,) = root.method_call(SC, "CreateSession", (sc_props,))?;
    let sp = conn.with_proxy(SC, sess_path.into_static(), T);
    let mut props = dbus::arg::PropMap::new();
    props.insert(
        "cursor-mode".into(),
        dbus::arg::Variant(Box::new(1u32) as Box<dyn dbus::arg::RefArg>),
    );
    let (stream_path,): (Path,) =
        sp.method_call(SESSION, "RecordMonitor", (conector.as_str(), props))?;
    let stream_path = stream_path.into_static();
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut regra = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    regra.path = Some(stream_path.clone());
    conn.add_match(regra, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    // Quem chama Start e a thread de rede, dona da sessao de input --
    // iniciar a de input inicia a de tela vinculada.
    tx_pronto.send(stream_path.to_string())?;
    let limite = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < limite {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id =
        (*node.lock().unwrap()).ok_or_else(|| anyhow::anyhow!("sem PipeWireStreamAdded"))?;

    // --- PipeWire + NVENC -----------------------------------------------
    pipewire::init();
    let mainloop = pipewire::main_loop::MainLoop::new(None)?;
    let context = pipewire::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let stream = Stream::new(
        &core,
        "logistream-host",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    struct Enc {
        sessao: Option<moq_nvenc::Session>,
        entrada: Option<moq_nvenc::Buffer>,
        saida: Option<moq_nvenc::Bitstream>,
        w: u32,
        h: u32,
        n: u64,
        descartados: u64,
    }
    let e = Rc::new(RefCell::new(Enc {
        sessao: None,
        entrada: None,
        saida: None,
        w: 0,
        h: 0,
        n: 0,
        descartados: 0,
    }));
    let e_fmt = Rc::clone(&e);
    let e_proc = Rc::clone(&e);

    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, pod| {
            if id != pipewire::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(pod) = pod else { return };
            let Ok((m, sub)) = pipewire::spa::param::format_utils::parse_format(pod) else {
                return;
            };
            if m != MediaType::Video || sub != MediaSubtype::Raw {
                return;
            }
            let mut info = pipewire::spa::param::video::VideoInfoRaw::default();
            if info.parse(pod).is_err() {
                return;
            }
            let (w, h) = (info.size().width, info.size().height);
            match montar_nvenc(w, h) {
                Ok((s, i, o)) => {
                    let mut g = e_fmt.borrow_mut();
                    g.w = w;
                    g.h = h;
                    g.sessao = Some(s);
                    g.entrada = Some(i);
                    g.saida = Some(o);
                    println!("  capturando {w}x{h}, NVENC pronto");
                }
                Err(err) => eprintln!("  NVENC: {err}"),
            }
        })
        .process(move |stream, ()| {
            let Some(mut buf) = stream.dequeue_buffer() else { return };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let stride = d.chunk().stride() as usize;
            let tam = d.chunk().size() as usize;
            if tam == 0 {
                return;
            }
            let Some(px) = d.data() else { return };
            let capturado = agora_ns();

            let mut g = e_proc.borrow_mut();
            if g.sessao.is_none() || g.entrada.is_none() || g.saida.is_none() {
                return;
            }
            let (w, h) = (g.w, g.h);
            let sessao = g.sessao.take().expect("conferido");
            let mut entrada = g.entrada.take().expect("conferido");
            let saida = g.saida.take().expect("conferido");

            {
                let Ok(mut lock) = entrada.lock() else {
                    g.sessao = Some(sessao);
                    return;
                };
                let pitch = lock.pitch() as usize;
                if pitch == stride {
                    unsafe { lock.write(&px[..tam.min(px.len())]) };
                } else {
                    let linha = (w as usize) * 4;
                    let mut junto = Vec::with_capacity(linha * h as usize);
                    for y in 0..h as usize {
                        let ini = y * stride;
                        if ini + linha <= px.len() {
                            junto.extend_from_slice(&px[ini..ini + linha]);
                        }
                    }
                    unsafe { lock.write(&junto) };
                }
            }

            let n = g.n;
            let r = sessao.encode_picture(
                entrada,
                saida,
                EncodePictureParams { input_timestamp: n, force_idr: n == 0 },
            );
            g.sessao = Some(sessao);
            match r.and_then(|s| s.finish()) {
                Ok((dados, i2, s2)) => {
                    g.entrada = Some(i2);
                    g.saida = Some(s2);
                    g.n += 1;
                    let keyframe = n == 0;
                    // `try_send`: se a rede nao acompanha, o quadro novo
                    // e descartado AQUI em vez de virar fila. Quadro
                    // atrasado nao tem valor numa tela remota.
                    if tx
                        .try_send(Codificado { dados, keyframe, captured_at_ns: capturado })
                        .is_err()
                    {
                        g.descartados += 1;
                    }
                }
                Err(err) => eprintln!("  encode: {err}"),
            }
        })
        .register()?;

    let obj = pod::object!(
        SpaTypes::ObjectParamFormat,
        pipewire::spa::param::ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA
        ),
    );
    let valores = pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&valores).ok_or_else(|| anyhow::anyhow!("pod"))?];
    stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    println!("  host na porta {porta}; ctrl-c para parar");
    mainloop.run();
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    let _ = rede.join();
    Ok(())
}

fn montar_nvenc(
    w: u32,
    h: u32,
) -> anyhow::Result<(moq_nvenc::Session, moq_nvenc::Buffer, moq_nvenc::Bitstream)> {
    Encoder::load().map_err(|e| anyhow::anyhow!("NVENC: {e:?}"))?;
    let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA: {e:?}"))?;
    let enc = Encoder::initialize_with_cuda(ctx).map_err(|e| anyhow::anyhow!("sessao: {e:?}"))?;
    let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
    let preset = enc
        .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
        .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
    let mut cfg = preset.presetCfg;
    // Sem isto, o cliente que entra no meio da transmissao -- o caso
    // normal -- nao tem cabecalho para comecar a desenhar.
    unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

    // --- qualidade -------------------------------------------------------
    //
    // O preset sozinho escolhia ~2 Mbps a 1440p, e o usuario viu o
    // resultado: "a imagem esta meio bugada, sem muita qualidade". Numa
    // LAN isso e banda sobrando na mesa -- o Parsec usa 20-50 Mbps.
    let pixels = (w as u64) * (h as u64);
    // ~8 Mbps em 1080p, ~15 em 1440p, ~34 em 4K. Escala com a area em
    // vez de ser numero fixo, porque 1440p e 4K nao cabem no mesmo teto.
    let alvo = (pixels * 60 / 1000).clamp(8_000_000, 60_000_000) as u32;
    cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
    cfg.rcParams.averageBitRate = alvo;
    // Teto com folga: tela parada gasta quase nada, e quando a imagem
    // muda inteira (rolar uma pagina) e exatamente a hora de gastar.
    cfg.rcParams.maxBitRate = alvo * 2;
    // VBV de um quadro: buffer grande deixa o encoder "dever" bits e
    // entregar quadro atrasado, que e o oposto do que tela remota quer.
    cfg.rcParams.vbvBufferSize = alvo / 60;

    // --- recuperacao de erro ---------------------------------------------
    //
    // So havia IDR no quadro zero. Perdeu um quadro, o erro NUNCA
    // cicatrizava -- ficava sujando a imagem para sempre. Um IDR a cada
    // 2s limita o estrago a 2s.
    // Piso de qualidade: `maxQP` limita o PIOR quadro. Sem ele, numa
    // cena que muda inteira (rolar uma pagina) o encoder joga a
    // qualidade no chao para caber no bitrate, e e justamente ali que a
    // pessoa esta olhando. 28 ainda e conservador para texto.
    cfg.rcParams.set_enableMaxQP(1);
    cfg.rcParams.maxQP.qpIntra = 28;
    cfg.rcParams.maxQP.qpInterP = 30;
    cfg.rcParams.maxQP.qpInterB = 32;
    cfg.gopLength = 120;
    // Sem quadros B: eles melhoram compressao a custo de atrasar a
    // saida, e aqui latencia vale mais que bitrate.
    cfg.frameIntervalP = 1;
    // Escrever em campo de uniao e seguro; so ler exige `unsafe`.
    cfg.encodeCodecConfig.h264Config.idrPeriod = 120;
    let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
    params
        .preset_guid(NV_ENC_PRESET_P4_GUID)
        .tuning_info(tuning)
        .framerate(60, 1)
        .enable_picture_type_decision();
    unsafe { params.encode_config(cfg) };
    let s = enc
        .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
        .map_err(|e| anyhow::anyhow!("start: {e}"))?;
    let i = s.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
    let o = s.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((s, i, o))
}

/// Injeta um evento na sessao do Mutter.
///
/// `NotifyPointerMotionAbsolute` recebe o caminho do STREAM de tela: e
/// ele que define o espaco de coordenadas. Por isso a sessao de input
/// precisa estar vinculada a de tela -- sem o vinculo, nao ha para onde
/// mapear o x,y.
#[cfg(target_os = "linux")]
fn aplicar(
    rd: &dbus::blocking::Proxy<'_, &Connection>,
    stream: &str,
    e: Evento,
) {
    use logistream::input::Evento as E;
    let r = match e {
        E::Mouse { x, y } => {
            rd.method_call::<(), _, _, _>(RD_SESSION, "NotifyPointerMotionAbsolute", (stream, x, y))
        }
        E::Botao { codigo, apertado } => {
            rd.method_call::<(), _, _, _>(RD_SESSION, "NotifyPointerButton", (codigo, apertado))
        }
        // flags 0 = rolagem continua. O `AxisDiscrete` seria para roda
        // com clique, mas o trackpad do Mac nao tem passo.
        E::Rolagem { dx, dy } => {
            rd.method_call::<(), _, _, _>(RD_SESSION, "NotifyPointerAxis", (dx, dy, 0u32))
        }
        E::Tecla { keycode, apertado } => {
            rd.method_call::<(), _, _, _>(RD_SESSION, "NotifyKeyboardKeycode", (keycode, apertado))
        }
    };
    if let Err(err) = r {
        eprintln!("  input recusado: {err}");
    }
}

/// Rede: serve video por datagrama e recebe input por stream.
///
/// `LocalSet` e nao `tokio::spawn` porque a conexao D-Bus **nao e
/// Send**, e a sessao de input pertence a conexao que a criou -- uma
/// segunda conexao chamando `Notify*` naquela sessao seria recusada pelo
/// Mutter. Entao tudo roda nesta thread e o `Rc` compartilha.
fn servir(
    porta: u16,
    mut rx: tokio::sync::mpsc::Receiver<Codificado>,
    tx_id: std::sync::mpsc::Sender<String>,
    rx_pronto: std::sync::mpsc::Receiver<String>,
) -> anyhow::Result<()> {
    use std::rc::Rc;

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let local = tokio::task::LocalSet::new();

    local.block_on(&rt, async move {
        let dbus = Rc::new(Connection::new_session()?);
        let rd_root = dbus.with_proxy(RD, RD_OBJ, T);
        let (rd_path,): (Path,) = rd_root.method_call(RD, "CreateSession", ())?;
        let rd_path = rd_path.into_static();
        let id: String = dbus.with_proxy(RD, rd_path.clone(), T).get(RD_SESSION, "SessionId")?;
        tx_id.send(id)?;

        // Espera a sessao de tela existir, ja vinculada a esta.
        let caminho_stream = rx_pronto.recv_timeout(Duration::from_secs(10))?;
        dbus.with_proxy(RD, rd_path.clone(), T)
            .method_call::<(), _, _, _>(RD_SESSION, "Start", ())?;
        println!("  input ligado (stream {caminho_stream})");

        let ep = net::servidor(([0, 0, 0, 0], porta).into())?;
        println!("  escutando em 0.0.0.0:{porta}");

        while let Some(entrando) = ep.accept().await {
            let conexao = entrando.await?;
            println!("  conectado: {}", conexao.remote_address());
            let Some(max) = conexao.max_datagram_size() else {
                eprintln!("  ERRO: o par nao aceita datagramas QUIC; nada de video vai passar");
                continue;
            };
            println!("  datagrama maximo: {max} bytes");

            // Input por stream unidirecional: perder um "tecla solta"
            // deixaria a tecla presa no host, entao nao pode ser
            // datagrama.
            let conexao_in = conexao.clone();
            let dbus_in = Rc::clone(&dbus);
            let rd_in = rd_path.clone();
            let stream_in = caminho_stream.clone();
            let entrada = tokio::task::spawn_local(async move {
                let Ok(mut recv) = conexao_in.accept_uni().await else { return };
                let rd = dbus_in.with_proxy(RD, rd_in, T);
                let mut leitor = Leitor::default();
                let mut buf = vec![0u8; 4096];
                let mut n_eventos = 0u64;
                while let Ok(Some(n)) = recv.read(&mut buf).await {
                    leitor.alimentar(&buf[..n]);
                    while let Some(e) = leitor.proximo() {
                        aplicar(&rd, &stream_in, e);
                        n_eventos += 1;
                        // Os primeiros, um a um: "nao deu erro" e prova
                        // fraca de que o input chegou. Depois so o
                        // agregado, para nao encher o log a 60 Hz.
                        if n_eventos <= 8 {
                            println!("  input #{n_eventos}: {e:?}");
                        } else if n_eventos % 500 == 0 {
                            println!("  {n_eventos} eventos de input aplicados");
                        }
                    }
                }
            });

            let mut quadro: u32 = 0;
            let mut enviados = 0u64;
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
            println!("  cliente saiu");

            // Sem ninguem assistindo, capturar e codificar so queima GPU
            // na maquina de outra pessoa. O host sobe sob demanda (o
            // daemon o acorda por SSH quando alguem clica), entao sair e
            // barato -- e e o unico jeito de ele nao virar processo
            // esquecido rodando a 80 fps a noite inteira.
            //
            // A janela de espera existe para reconexao: cair o wifi por
            // dois segundos nao deveria exigir voltar ao painel.
            println!("  aguardando {OCIOSO_S}s por outro cliente antes de sair");
            let volta = tokio::time::timeout(
                Duration::from_secs(OCIOSO_S),
                ep.accept(),
            )
            .await;
            if volta.is_ok() {
                // Reconectou dentro da janela. Nao da para devolver a
                // conexao ao laco de fora sem duplicar a logica, entao
                // saimos e o daemon religa no proximo clique -- custa um
                // clique, nao uma sessao travada.
                println!("  alguem voltou, mas a conexao nao volta para o laco; encerrando");
            } else {
                println!("  ninguem voltou; encerrando");
            }
            // `exit` e nao `break`: sair deste laco encerraria so a
            // thread de rede, e o laco do PipeWire na thread principal
            // seguiria capturando para sempre -- exatamente o processo
            // esquecido que esta saida existe para evitar. Nao ha estado
            // a salvar: o Mutter solta as sessoes quando o par sai do
            // barramento.
            std::process::exit(0);
        }
        Ok::<(), anyhow::Error>(())
    })
}
