//! Screen capture on Wayland: Mutter negotiates, PipeWire delivers.
//!
//! **Through Mutter's private API, not the portal.** The `ScreenCast` portal
//! requires interactive approval -- it would open a dialog on the screen of
//! the captured machine, which is exactly the machine where there may be
//! nobody. `org.gnome.Mutter.ScreenCast` is what the portal itself uses
//! underneath, and it asks nothing.
//!
//! Same precedent as the edge: `border.rs` talks to
//! `org.gnome.Mutter.InputCapture` directly because the equivalent portal
//! accepts `CreateSession` and never answers.
//!
//!   cargo run --release --bin capture-probe -- [frames] [connector]

// The whole file is Linux -- one `cfg` over the group, not one per
// line. The stub `main` below is the only thing that survives on
// another target.
#[cfg(target_os = "linux")]
use {
    std::cell::RefCell,
    std::rc::Rc,
    std::time::{Duration, Instant},

    dbus::blocking::stdintf::org_freedesktop_dbus::Properties,
    dbus::blocking::Connection,
    dbus::message::MatchRule,
    dbus::Path,

    pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType},
    pipewire::spa::param::video::VideoFormat,
    pipewire::spa::pod::{self, Pod},
    pipewire::spa::sys,
    pipewire::spa::utils::{Direction, SpaTypes},
    pipewire::stream::{Stream, StreamFlags},
};

#[cfg(target_os = "linux")]
const SC: &str = "org.gnome.Mutter.ScreenCast";
#[cfg(target_os = "linux")]
const SC_OBJ: &str = "/org/gnome/Mutter/ScreenCast";
#[cfg(target_os = "linux")]
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";
#[cfg(target_os = "linux")]
const STREAM: &str = "org.gnome.Mutter.ScreenCast.Stream";
#[cfg(target_os = "linux")]
const T: Duration = Duration::from_secs(10);

#[cfg(target_os = "linux")]
#[derive(Default)]
struct State {
    frames: u64,
    bytes: u64,
    width: u32,
    height: u32,
    format: String,
    first: Option<Instant>,
    last: Option<Instant>,
    intervals_ms: Vec<f64>,
}

// Only on Linux, and the stub says so instead of the crate
// failing to build: the probe talks to Mutter over D-Bus and takes
// frames over PipeWire, neither of which exists on Windows. Same
// shape as `kvm.rs`, which solved this first -- and it is what lets
// check.sh cross-build the WHOLE crate instead of a hand-picked
// list of binaries.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("logistream-probe-capture so roda no Linux: ele fala com o Mutter por D-Bus e recebe os quadros por PipeWire");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let target: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(120);
    let connector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());

    // --- 1. Mutter: negotiate and discover the node ---------------------
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
        sp.method_call(SESSION, "RecordMonitor", (connector.as_str(), props))?;
    let stream_path = stream_path.into_static();

    // Register BEFORE Start: the signal arrives right after, and listening
    // only afterwards is a lost race.
    // Arc+Mutex and not Rc: the dbus callback requires `Send`.
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut rule = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    rule.path = Some(stream_path.clone());
    conn.add_match(rule, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    sp.method_call::<(), _, _, _>(SESSION, "Start", ())?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < deadline {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id = (*node.lock().unwrap()).ok_or_else(|| {
        anyhow::anyhow!("Mutter did not emit PipeWireStreamAdded within 5s")
    })?;

    let stp = conn.with_proxy(SC, stream_path, T);
    let params: dbus::arg::PropMap = stp.get(STREAM, "Parameters").unwrap_or_default();
    println!("  node {node_id}, {} ({connector})",
        params.get("size").map(|v| format!("{v:?}")).unwrap_or_default());

    // The D-Bus connection stays alive until the end: Mutter closes the
    // session when the peer leaves the bus, and then the node evaporates
    // mid-capture.

    // --- 2. PipeWire: consume the frames --------------------------------
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

    let state = Rc::new(RefCell::new(State::default()));
    let loop_ = mainloop.clone();
    let st_fmt = Rc::clone(&state);
    let st_proc = Rc::clone(&state);

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
                let mut e = st_fmt.borrow_mut();
                e.width = info.size().width;
                e.height = info.size().height;
                e.format = format!("{:?}", info.format());
                println!("  negotiated format: {}x{} {}", e.width, e.height, e.format);
            }
        })
        .process(move |stream, ()| {
            let Some(mut buf) = stream.dequeue_buffer() else { return };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let size = d.chunk().size() as u64;
            if size == 0 {
                return; // empty frame: this is how PipeWire signals that nothing changed
            }
            let now = Instant::now();
            let mut e = st_proc.borrow_mut();
            if let Some(prev) = e.last {
                e.intervals_ms.push((now - prev).as_secs_f64() * 1000.0);
            }
            e.last = Some(now);
            e.first.get_or_insert(now);
            e.frames += 1;
            e.bytes += size;
            if e.frames >= target {
                loop_.quit();
            }
        })
        .register()?;

    // We ask for BGRx and RGBx: that is what Mutter delivers and what NVENC
    // consumes without color conversion on the CPU, which would be the
    // obvious bottleneck.
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
    let values = pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or_else(|| anyhow::anyhow!("invalid pod"))?];

    stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let t0 = Instant::now();
    mainloop.run();

    let e = state.borrow();
    let dur = e.first.map(|p| (Instant::now() - p).as_secs_f64()).unwrap_or(0.0);
    println!("  {} frames in {:.2}s = {:.1} fps", e.frames, dur, e.frames as f64 / dur.max(0.001));
    if !e.intervals_ms.is_empty() {
        let mut v = e.intervals_ms.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("  interval between frames: p50 {:.1}ms  p95 {:.1}ms",
            v[v.len() / 2], v[v.len() * 95 / 100]);
    }
    println!("  {:.1} MB raw ({} bytes/frame)", e.bytes as f64 / 1_048_576.0,
        e.bytes.checked_div(e.frames).unwrap_or(0));
    println!("  negotiation + capture in {:.2}s", t0.elapsed().as_secs_f64());
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
use sys as _;
