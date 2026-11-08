//! Full v1: screen -> NVENC -> .h264 file.
//!
//! Combines what the two probes proved separately. Separate on purpose
//! until now: mixing capture and encoding would make the failure of one
//! look like the other's.
//!
//! What this measurement answers: **does it fit the budget?** At 60 fps
//! there are 16.6 ms per frame to capture, copy and encode. If it does
//! not fit, no choice of transport will save it.
//!
//!   cargo run --release --bin v1-record -- [frames] [connector]

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cudarc::driver::CudaContext;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use dbus::Path;
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
const T: Duration = Duration::from_secs(10);

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let target: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(180);
    let connector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());

    // --- Mutter ---------------------------------------------------------
    let conn = Connection::new_session()?;
    let root = conn.with_proxy(SC, SC_OBJ, T);
    let (sess_path,): (Path,) =
        root.method_call(SC, "CreateSession", (dbus::arg::PropMap::new(),))?;
    let sp = conn.with_proxy(SC, sess_path.into_static(), T);
    let mut props = dbus::arg::PropMap::new();
    props.insert(
        "cursor-mode".into(),
        dbus::arg::Variant(Box::new(1u32) as Box<dyn dbus::arg::RefArg>),
    );
    let (stream_path,): (Path,) =
        sp.method_call(SESSION, "RecordMonitor", (connector.as_str(), props))?;
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut rule = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    rule.path = Some(stream_path.into_static());
    conn.add_match(rule, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    sp.method_call::<(), _, _, _>(SESSION, "Start", ())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < deadline {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id =
        (*node.lock().unwrap()).ok_or_else(|| anyhow::anyhow!("no PipeWireStreamAdded"))?;

    // --- PipeWire -------------------------------------------------------
    pipewire::init();
    let mainloop = pipewire::main_loop::MainLoop::new(None)?;
    let context = pipewire::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let stream = Stream::new(
        &core,
        "oxdesk-v1",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    /// The encoder can only be born after the format is negotiated: the
    /// NVENC session fixes width and height at creation.
    struct Pipeline {
        session: Option<moq_nvenc::Session>,
        input: Option<moq_nvenc::Buffer>,
        output: Option<moq_nvenc::Bitstream>,
        file: std::fs::File,
        w: u32,
        h: u32,
        n: u64,
        bytes: u64,
        copy_ms: Vec<f64>,
        enc_ms: Vec<f64>,
        total_ms: Vec<f64>,
    }

    let dest = std::env::temp_dir().join("oxdesk-v1.h264");
    let p = Rc::new(RefCell::new(Pipeline {
        session: None,
        input: None,
        output: None,
        file: std::fs::File::create(&dest)?,
        w: 0,
        h: 0,
        n: 0,
        bytes: 0,
        copy_ms: Vec::new(),
        enc_ms: Vec::new(),
        total_ms: Vec::new(),
    }));

    let p_fmt = Rc::clone(&p);
    let p_proc = Rc::clone(&p);
    let loop_ = mainloop.clone();

    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |_, _, id, pod| {
            if id != pipewire::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(pod) = pod else { return };
            let Ok((media, sub)) = pipewire::spa::param::format_utils::parse_format(pod) else {
                return;
            };
            if media != MediaType::Video || sub != MediaSubtype::Raw {
                return;
            }
            let mut info = pipewire::spa::param::video::VideoInfoRaw::default();
            if info.parse(pod).is_err() {
                return;
            }
            let (w, h) = (info.size().width, info.size().height);
            println!("  format: {w}x{h} {:?}", info.format());

            // NVENC only now, once we know the size.
            let build = || -> anyhow::Result<_> {
                Encoder::load().map_err(|e| anyhow::anyhow!("NVENC: {e:?}"))?;
                let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA: {e:?}"))?;
                let enc = Encoder::initialize_with_cuda(ctx)
                    .map_err(|e| anyhow::anyhow!("session: {e:?}"))?;
                let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
                let preset = enc
                    .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
                    .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
        let mut cfg = preset.presetCfg;
        // Without this, a client that joins mid-stream -- the normal
        // case -- has no header to start drawing from.
        unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

        // --- quality ---------------------------------------------------------
        //
        // The preset alone picked ~2 Mbps at 1440p, and the user saw the
        // result: "the picture is kind of glitchy, not much quality". On a
        // LAN that is bandwidth left on the table -- Parsec uses 20-50 Mbps.
        let pixels = (w as u64) * (h as u64);
        // ~8 Mbps at 1080p, ~15 at 1440p, ~34 at 4K. Scales with area
        // instead of being a fixed number, because 1440p and 4K do not fit
        // under the same ceiling.
        let bitrate = (pixels * 60 / 1000).clamp(8_000_000, 60_000_000) as u32;
        cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
        cfg.rcParams.averageBitRate = bitrate;
        // Generous ceiling: a still screen spends almost nothing, and when
        // the whole picture changes (scrolling a page) is exactly the time
        // to spend.
        cfg.rcParams.maxBitRate = bitrate * 2;
        // One-frame VBV: a large buffer lets the encoder "owe" bits and
        // deliver a late frame, the opposite of what a remote screen wants.
        cfg.rcParams.vbvBufferSize = bitrate / 60;

        // --- error recovery --------------------------------------------------
        //
        // There was only an IDR at frame zero. Lose a frame and the error
        // NEVER healed -- it kept smearing the picture forever. One IDR
        // every 2s limits the damage to 2s.
        cfg.gopLength = 120;
        // No B-frames: they improve compression at the cost of delaying
        // output, and here latency is worth more than bitrate.
        cfg.frameIntervalP = 1;
                cfg.encodeCodecConfig.h264Config.idrPeriod = 120;
                let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
                params
                    .preset_guid(NV_ENC_PRESET_P4_GUID)
                    .tuning_info(tuning)
                    .framerate(60, 1)
                    .enable_picture_type_decision();
                unsafe { params.encode_config(cfg) };
                let session = enc
                    .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
                    .map_err(|e| anyhow::anyhow!("start: {e}"))?;
                let input = session.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
                let output = session.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok((session, input, output))
            };
            match build() {
                Ok((s, e, o)) => {
                    let mut g = p_fmt.borrow_mut();
                    g.w = w;
                    g.h = h;
                    g.session = Some(s);
                    g.input = Some(e);
                    g.output = Some(o);
                    println!("  NVENC ready for {w}x{h}");
                }
                Err(e) => eprintln!("  NVENC failed: {e}"),
            }
        })
        .process(move |stream, ()| {
            let Some(mut buf) = stream.dequeue_buffer() else { return };
            let datas = buf.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let stride = d.chunk().stride() as usize;
            let size = d.chunk().size() as usize;
            if size == 0 {
                return;
            }
            let Some(px) = d.data() else { return };

            let t_frame = Instant::now();
            let mut g = p_proc.borrow_mut();
            let (w, h) = (g.w, g.h);
            // Takes all three out of the struct at once: the borrow checker
            // does not allow borrowing one immutably and moving another in
            // the same expression, and `encode_picture` consumes the
            // buffers and hands them back.
            if g.session.is_none() || g.input.is_none() || g.output.is_none() {
                return;
            }
            let session = g.session.take().expect("checked above");
            let input = g.input.take().expect("checked above");
            let output = g.output.take().expect("checked above");

            // PipeWire's BGRx == NVENC's ARGB in memory (B,G,R,A).
            // Same byte order, so the copy is raw -- no color conversion on
            // the CPU, which would be the obvious bottleneck.
            let t_copy = Instant::now();
            let mut input = input;
            {
                let Ok(mut lock) = input.lock() else { return };
                let pitch = lock.pitch() as usize;
                if pitch == stride {
                    unsafe { lock.write(&px[..size.min(px.len())]) };
                } else {
                    // Row by row when NVENC's pitch differs from the
                    // capture's. It happens: NVENC aligns, the compositor
                    // does not.
                    let row = (w as usize) * 4;
                    let mut packed = Vec::with_capacity(row * h as usize);
                    for y in 0..h as usize {
                        let start = y * stride;
                        if start + row <= px.len() {
                            packed.extend_from_slice(&px[start..start + row]);
                        }
                    }
                    unsafe { lock.write(&packed) };
                }
            }
            let copy_ms = t_copy.elapsed().as_secs_f64() * 1000.0;

            let t_enc = Instant::now();
            let n = g.n;
            let r = session.encode_picture(
                input,
                output,
                EncodePictureParams { input_timestamp: n, force_idr: n == 0 },
            );
            match r.and_then(|s| s.finish()) {
                Ok((data, e2, s2)) => {
                    let enc_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
                    g.session = Some(session);
                    let _ = g.file.write_all(&data);
                    g.bytes += data.len() as u64;
                    g.input = Some(e2);
                    g.output = Some(s2);
                    g.n += 1;
                    g.copy_ms.push(copy_ms);
                    g.enc_ms.push(enc_ms);
                    g.total_ms.push(t_frame.elapsed().as_secs_f64() * 1000.0);
                    if g.n >= target {
                        loop_.quit();
                    }
                }
                Err(e) => {
                    g.session = Some(session);
                    eprintln!("  encode failed: {e}");
                }
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
    let values = pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or_else(|| anyhow::anyhow!("pod"))?];
    stream.connect(
        Direction::Input,
        Some(node_id),
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;

    let t0 = Instant::now();
    mainloop.run();
    let dur = t0.elapsed().as_secs_f64();

    let g = p.borrow();
    let pct = |v: &Vec<f64>, q: usize| {
        let mut s = v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s.get(s.len() * q / 100).copied().unwrap_or(0.0)
    };
    println!("\n  {} frames {}x{} in {dur:.2}s = {:.1} fps",
        g.n, g.w, g.h, g.n as f64 / dur.max(0.001));
    println!("  copy:   p50 {:.2}ms  p95 {:.2}ms", pct(&g.copy_ms, 50), pct(&g.copy_ms, 95));
    println!("  encode: p50 {:.2}ms  p95 {:.2}ms", pct(&g.enc_ms, 50), pct(&g.enc_ms, 95));
    println!("  total:  p50 {:.2}ms  p95 {:.2}ms  (budget 16.6ms at 60fps)",
        pct(&g.total_ms, 50), pct(&g.total_ms, 95));
    println!("  {:.1} MB = {:.1} Mbps at 60fps",
        g.bytes as f64 / 1_048_576.0,
        (g.bytes as f64 * 8.0 / g.n.max(1) as f64) * 60.0 / 1_000_000.0);
    println!("  {}", dest.display());
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    Ok(())
}
