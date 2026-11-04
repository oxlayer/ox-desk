//! Captura de tela no Wayland: Mutter negocia, PipeWire entrega.
//!
//! **Pela API privada do Mutter, nao pelo portal.** O portal
//! `ScreenCast` exige aprovacao interativa -- abriria um dialogo na tela
//! da maquina capturada, que e exatamente a maquina onde pode nao haver
//! ninguem. O `org.gnome.Mutter.ScreenCast` e o que o proprio portal usa
//! por baixo e nao pergunta nada.
//!
//! Mesmo precedente da borda: o `border.rs` fala com o
//! `org.gnome.Mutter.InputCapture` direto porque o portal equivalente
//! aceita `CreateSession` e nunca responde.
//!
//!   cargo run --release --bin capture-probe -- [quadros] [conector]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use dbus::Path;

use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::video::VideoFormat;
use pipewire::spa::pod::{self, Pod};
use pipewire::spa::sys;
use pipewire::spa::utils::{Direction, SpaTypes};
use pipewire::stream::{Stream, StreamFlags};

const SC: &str = "org.gnome.Mutter.ScreenCast";
const SC_OBJ: &str = "/org/gnome/Mutter/ScreenCast";
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";
const STREAM: &str = "org.gnome.Mutter.ScreenCast.Stream";
const T: Duration = Duration::from_secs(10);

#[derive(Default)]
struct Estado {
    quadros: u64,
    bytes: u64,
    largura: u32,
    altura: u32,
    formato: String,
    primeiro: Option<Instant>,
    ultimo: Option<Instant>,
    intervalos_ms: Vec<f64>,
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let alvo: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(120);
    let conector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());

    // --- 1. Mutter: negociar e descobrir o node -------------------------
    let conn = Connection::new_session()?;
    let root = conn.with_proxy(SC, SC_OBJ, T);
    let (sess_path,): (Path,) =
        root.method_call(SC, "CreateSession", (dbus::arg::PropMap::new(),))?;
    let sess_path = sess_path.into_static();

    let sp = conn.with_proxy(SC, sess_path.clone(), T);
    let mut props = dbus::arg::PropMap::new();
    props.insert(
        "cursor-mode".into(),
        dbus::arg::Variant(Box::new(1u32) as Box<dyn dbus::arg::RefArg>),
    );
    let (stream_path,): (Path,) =
        sp.method_call(SESSION, "RecordMonitor", (conector.as_str(), props))?;
    let stream_path = stream_path.into_static();

    // Registrar ANTES do Start: o sinal chega logo depois, e ouvir so
    // depois e corrida perdida.
    // Arc+Mutex e nao Rc: o callback do dbus exige `Send`.
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut regra = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    regra.path = Some(stream_path.clone());
    conn.add_match(regra, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    sp.method_call::<(), _, _, _>(SESSION, "Start", ())?;

    let limite = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < limite {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id = (*node.lock().unwrap()).ok_or_else(|| {
        anyhow::anyhow!("o Mutter nao emitiu PipeWireStreamAdded em 5s")
    })?;

    let stp = conn.with_proxy(SC, stream_path, T);
    let params: dbus::arg::PropMap = stp.get(STREAM, "Parameters").unwrap_or_default();
    println!("  node {node_id}, {} ({conector})",
        params.get("size").map(|v| format!("{v:?}")).unwrap_or_default());

    // A conexao D-Bus fica viva ate o fim: o Mutter fecha a sessao quando
    // o par some do barramento, e ai o node evapora no meio da captura.

    // --- 2. PipeWire: consumir os quadros -------------------------------
    pipewire::init();
    let mainloop = pipewire::main_loop::MainLoop::new(None)?;
    let context = pipewire::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let stream = Stream::new(
        &core,
        "logistream-capture",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let estado = Rc::new(RefCell::new(Estado::default()));
    let loop_ = mainloop.clone();
    let est_fmt = Rc::clone(&estado);
    let est_proc = Rc::clone(&estado);

    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, pod| {
            if id != pipewire::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(pod) = pod else { return };
            let Ok((media, subtype)) = pipewire::spa::param::format_utils::parse_format(pod) else {
                return;
            };
            if media != MediaType::Video || subtype != MediaSubtype::Raw {
                return;
            }
            let mut info = pipewire::spa::param::video::VideoInfoRaw::default();
            if info.parse(pod).is_ok() {
                let mut e = est_fmt.borrow_mut();
                e.largura = info.size().width;
                e.altura = info.size().height;
                e.formato = format!("{:?}", info.format());
                println!("  formato negociado: {}x{} {}", e.largura, e.altura, e.formato);
            }
        })
        .process(move |stream, ()| {
            let Some(mut buf) = stream.dequeue_buffer() else { return };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let tamanho = d.chunk().size() as u64;
            if tamanho == 0 {
                return; // quadro vazio: o PipeWire sinaliza assim quando nada mudou
            }
            let agora = Instant::now();
            let mut e = est_proc.borrow_mut();
            if let Some(ant) = e.ultimo {
                e.intervalos_ms.push((agora - ant).as_secs_f64() * 1000.0);
            }
            e.ultimo = Some(agora);
            e.primeiro.get_or_insert(agora);
            e.quadros += 1;
            e.bytes += tamanho;
            if e.quadros >= alvo {
                loop_.quit();
            }
        })
        .register()?;

    // Pedimos BGRx e RGBx: e o que o Mutter entrega e o que o NVENC
    // consome sem conversao de cor na CPU, que seria o gargalo obvio.
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
            VideoFormat::RGBx,
            VideoFormat::BGRA,
            VideoFormat::RGBA
        ),
    );
    let valores = pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&valores).ok_or_else(|| anyhow::anyhow!("pod invalido"))?];

    stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let t0 = Instant::now();
    mainloop.run();

    let e = estado.borrow();
    let dur = e.primeiro.map(|p| (Instant::now() - p).as_secs_f64()).unwrap_or(0.0);
    println!("  {} quadros em {:.2}s = {:.1} fps", e.quadros, dur, e.quadros as f64 / dur.max(0.001));
    if !e.intervalos_ms.is_empty() {
        let mut v = e.intervalos_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("  intervalo entre quadros: p50 {:.1}ms  p95 {:.1}ms",
            v[v.len() / 2], v[v.len() * 95 / 100]);
    }
    println!("  {:.1} MB crus ({} bytes/quadro)", e.bytes as f64 / 1_048_576.0,
        e.bytes.checked_div(e.quadros).unwrap_or(0));
    println!("  negociacao + captura em {:.2}s", t0.elapsed().as_secs_f64());
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    Ok(())
}

#[allow(unused_imports)]
use sys as _;
