//! Host no Linux/Wayland: Mutter -> PipeWire -> NVENC -> QUIC.
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

#![cfg(target_os = "linux")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use dbus::Path;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::video::VideoFormat;
use pipewire::spa::pod::{self, Pod};
use pipewire::spa::utils::{Direction, SpaTypes};
use pipewire::stream::{Stream, StreamFlags};

use crate::input::{Evento, Leitor};
use crate::nvenc::Codificador;
use crate::serve::{self, agora_ns, Quadro, Sinais};

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

pub fn executar(porta: u16, conector: &str) -> anyhow::Result<()> {
    // Canal do TOKIO, nao `std::sync::mpsc`.
    //
    // Com o canal da std, o laco de rede bloqueava a thread em
    // `recv_timeout` -- e numa runtime `current_thread` isso impede o
    // driver da conexao de rodar. `send_datagram` so ENFILEIRA; quem poe
    // no fio e o driver. Resultado medido: host anunciando 80 fps e "0
    // recusados" com o receptor em zero, porque nada era transmitido.
    let (tx, rx) = tokio::sync::mpsc::channel::<Quadro>(CAPACIDADE);

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

    // Quem decide encerrar e a rede (ninguem assistindo), mas quem pode
    // PARAR e a thread de captura -- e a dona do laco do PipeWire e dos
    // recursos de CUDA/NVENC.
    //
    // Tentei `std::process::exit` da thread de rede e o processo travou:
    // o exit comeca a destruir os estaticos enquanto a captura ainda
    // esta codificando, e o log mostrou `encode: InvalidParam` seguido de
    // nada. Bandeira + timer no proprio laco resolve na ordem certa.
    let sinais = std::sync::Arc::new(Sinais::default());
    let sinais_rede = std::sync::Arc::clone(&sinais);

    let rede = std::thread::spawn(move || {
        if let Err(e) = servir(porta, rx, tx_id, rx_pronto, sinais_rede) {
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
        sp.method_call(SESSION, "RecordMonitor", (conector, props))?;
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

    let cod = Rc::new(RefCell::new(None::<Codificador>));
    let descartados = Rc::new(Cell::new(0u64));
    let cod_fmt = Rc::clone(&cod);
    let cod_proc = Rc::clone(&cod);
    let desc_proc = Rc::clone(&descartados);
    let sinais_proc = std::sync::Arc::clone(&sinais);

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
            match Codificador::novo(w, h) {
                Ok(c) => {
                    *cod_fmt.borrow_mut() = Some(c);
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

            // Sem ninguem assistindo, nao codifica: o quadro iria para
            // um canal que ninguem drena. O buffer do PipeWire ja foi
            // devolvido acima, entao a captura segue saudavel.
            if !sinais_proc.tem_cliente() {
                return;
            }
            let mut g = cod_proc.borrow_mut();
            let Some(c) = g.as_mut() else { return };
            if sinais_proc.pegar_idr() {
                c.pedir_idr();
            }
            match c.codificar(px, stride) {
                // `try_send`: se a rede nao acompanha, o quadro novo e
                // descartado AQUI em vez de virar fila. Quadro atrasado
                // nao tem valor numa tela remota.
                Ok(dados) => {
                    let keyframe = c.ultimo_foi_idr();
                    if tx.try_send(Quadro { dados, keyframe, captured_at_ns: capturado }).is_err() {
                        desc_proc.set(desc_proc.get() + 1);
                    }
                }
                Err(err) => eprintln!("  {err}"),
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

    // Timer no proprio laco: e o unico lugar de onde da para chamar
    // `quit` com seguranca, porque o `MainLoop` nao atravessa threads.
    let ml = mainloop.clone();
    let sinais_timer = std::sync::Arc::clone(&sinais);
    let timer = mainloop.loop_().add_timer(move |_| {
        if sinais_timer.deve_sair() {
            ml.quit();
        }
    });
    timer
        .update_timer(Some(Duration::from_millis(500)), Some(Duration::from_millis(500)))
        .into_result()?;

    println!("  host na porta {porta}; ctrl-c para parar");
    mainloop.run();
    println!("  captura encerrada");
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    let _ = rede.join();
    Ok(())
}

/// Injeta um evento na sessao do Mutter.
///
/// `NotifyPointerMotionAbsolute` recebe o caminho do STREAM de tela: e
/// ele que define o espaco de coordenadas. Por isso a sessao de input
/// precisa estar vinculada a de tela -- sem o vinculo, nao ha para onde
/// mapear o x,y.
fn aplicar(rd: &dbus::blocking::Proxy<'_, &Connection>, stream: &str, e: Evento) {
    use crate::input::Evento as E;
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
/// `LocalSet` e nao `tokio::spawn` porque a conexao D-Bus **nao e Send**,
/// e a sessao de input pertence a conexao que a criou -- uma segunda
/// conexao chamando `Notify*` naquela sessao seria recusada pelo Mutter.
/// Entao tudo roda nesta thread e o `Rc` compartilha.
fn servir(
    porta: u16,
    mut rx: tokio::sync::mpsc::Receiver<Quadro>,
    tx_id: std::sync::mpsc::Sender<String>,
    rx_pronto: std::sync::mpsc::Receiver<String>,
    sinais: std::sync::Arc<Sinais>,
) -> anyhow::Result<()> {
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

        let ep = crate::net::servidor(([0, 0, 0, 0], porta).into())?;
        println!("  escutando em 0.0.0.0:{porta}");

        serve::atender(&ep, &mut rx, &sinais, |conexao| {
            let conexao = conexao.clone();
            let dbus = Rc::clone(&dbus);
            let rd_in = rd_path.clone();
            let stream_in = caminho_stream.clone();
            tokio::task::spawn_local(async move {
                let Ok(mut recv) = conexao.accept_uni().await else { return };
                let rd = dbus.with_proxy(RD, rd_in, T);
                let mut leitor = Leitor::default();
                let mut buf = vec![0u8; 4096];
                let mut n = 0u64;
                while let Ok(Some(lidos)) = recv.read(&mut buf).await {
                    leitor.alimentar(&buf[..lidos]);
                    while let Some(e) = leitor.proximo() {
                        aplicar(&rd, &stream_in, e);
                        n += 1;
                        // Os primeiros, um a um: "nao deu erro" e prova
                        // fraca de que o input chegou. Depois so o
                        // agregado, para nao encher o log a 60 Hz.
                        if n <= 8 {
                            println!("  input #{n}: {e:?}");
                        } else if n % 500 == 0 {
                            println!("  {n} eventos de input aplicados");
                        }
                    }
                }
            })
        })
        .await
    })
}
