//! v1 completa: tela -> NVENC -> arquivo .h264.
//!
//! Junta o que os dois probes provaram separados. Separados de proposito
//! ate aqui: misturar captura e codificacao faria o fracasso de uma
//! parecer o da outra.
//!
//! O que esta medicao responde: **cabe no orcamento?** A 60 fps ha 16,6 ms
//! por quadro para capturar, copiar e codificar. Se nao couber, nenhuma
//! escolha de transporte salva.
//!
//!   cargo run --release --bin v1-record -- [quadros] [conector]

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cudarc::driver::CudaContext;
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use dbus::Path;
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, NV_ENC_TUNING_INFO,
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
    let alvo: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(180);
    let conector = a.get(1).cloned().unwrap_or_else(|| "DP-1".into());

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
        sp.method_call(SESSION, "RecordMonitor", (conector.as_str(), props))?;
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut regra = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    regra.path = Some(stream_path.into_static());
    conn.add_match(regra, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    sp.method_call::<(), _, _, _>(SESSION, "Start", ())?;
    let limite = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < limite {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id =
        (*node.lock().unwrap()).ok_or_else(|| anyhow::anyhow!("sem PipeWireStreamAdded"))?;

    // --- PipeWire -------------------------------------------------------
    pipewire::init();
    let mainloop = pipewire::main_loop::MainLoop::new(None)?;
    let context = pipewire::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let stream = Stream::new(
        &core,
        "logistream-v1",
        pipewire::properties::properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    /// O encoder so pode nascer depois de o formato ser negociado: a
    /// sessao NVENC fixa largura e altura na criacao.
    struct Pipeline {
        sessao: Option<moq_nvenc::Session>,
        entrada: Option<moq_nvenc::Buffer>,
        saida: Option<moq_nvenc::Bitstream>,
        arquivo: std::fs::File,
        w: u32,
        h: u32,
        n: u64,
        bytes: u64,
        copia_ms: Vec<f64>,
        enc_ms: Vec<f64>,
        total_ms: Vec<f64>,
    }

    let destino = std::env::temp_dir().join("logistream-v1.h264");
    let p = Rc::new(RefCell::new(Pipeline {
        sessao: None,
        entrada: None,
        saida: None,
        arquivo: std::fs::File::create(&destino)?,
        w: 0,
        h: 0,
        n: 0,
        bytes: 0,
        copia_ms: Vec::new(),
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
            println!("  formato: {w}x{h} {:?}", info.format());

            // NVENC so agora, que ja sabemos o tamanho.
            let montar = || -> anyhow::Result<_> {
                Encoder::load().map_err(|e| anyhow::anyhow!("NVENC: {e:?}"))?;
                let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA: {e:?}"))?;
                let enc = Encoder::initialize_with_cuda(ctx)
                    .map_err(|e| anyhow::anyhow!("sessao: {e:?}"))?;
                let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
                let preset = enc
                    .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
                    .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
                let mut cfg = preset.presetCfg;
                // Sem isto o cliente que entra no meio nao tem cabecalho
                // para comecar a desenhar. Ver specs/010.
                unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };
                let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
                params
                    .preset_guid(NV_ENC_PRESET_P4_GUID)
                    .tuning_info(tuning)
                    .framerate(60, 1)
                    .enable_picture_type_decision();
                unsafe { params.encode_config(cfg) };
                let sessao = enc
                    .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
                    .map_err(|e| anyhow::anyhow!("start: {e}"))?;
                let entrada = sessao.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
                let saida = sessao.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok((sessao, entrada, saida))
            };
            match montar() {
                Ok((s, e, o)) => {
                    let mut g = p_fmt.borrow_mut();
                    g.w = w;
                    g.h = h;
                    g.sessao = Some(s);
                    g.entrada = Some(e);
                    g.saida = Some(o);
                    println!("  NVENC pronto para {w}x{h}");
                }
                Err(e) => eprintln!("  NVENC falhou: {e}"),
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

            let t_quadro = Instant::now();
            let mut g = p_proc.borrow_mut();
            let (w, h) = (g.w, g.h);
            // Tira os tres da struct de uma vez: o borrow checker nao
            // deixa emprestar um imutavel e mover outro na mesma
            // expressao, e `encode_picture` consome os buffers e os
            // devolve.
            if g.sessao.is_none() || g.entrada.is_none() || g.saida.is_none() {
                return;
            }
            let sessao = g.sessao.take().expect("conferido acima");
            let entrada = g.entrada.take().expect("conferido acima");
            let saida = g.saida.take().expect("conferido acima");

            // BGRx do PipeWire == ARGB do NVENC na memoria (B,G,R,A).
            // Mesma ordem de bytes, entao a copia e crua -- nenhuma
            // conversao de cor na CPU, que seria o gargalo obvio.
            let t_copia = Instant::now();
            let mut entrada = entrada;
            {
                let Ok(mut lock) = entrada.lock() else { return };
                let pitch = lock.pitch() as usize;
                if pitch == stride {
                    unsafe { lock.write(&px[..tam.min(px.len())]) };
                } else {
                    // Linha a linha quando o passo do NVENC difere do da
                    // captura. Acontece: o NVENC alinha, o compositor nao.
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
            let copia_ms = t_copia.elapsed().as_secs_f64() * 1000.0;

            let t_enc = Instant::now();
            let n = g.n;
            let r = sessao.encode_picture(
                entrada,
                saida,
                EncodePictureParams { input_timestamp: n, force_idr: n == 0 },
            );
            match r.and_then(|s| s.finish()) {
                Ok((dados, e2, s2)) => {
                    let enc_ms = t_enc.elapsed().as_secs_f64() * 1000.0;
                    g.sessao = Some(sessao);
                    let _ = g.arquivo.write_all(&dados);
                    g.bytes += dados.len() as u64;
                    g.entrada = Some(e2);
                    g.saida = Some(s2);
                    g.n += 1;
                    g.copia_ms.push(copia_ms);
                    g.enc_ms.push(enc_ms);
                    g.total_ms.push(t_quadro.elapsed().as_secs_f64() * 1000.0);
                    if g.n >= alvo {
                        loop_.quit();
                    }
                }
                Err(e) => {
                    g.sessao = Some(sessao);
                    eprintln!("  encode falhou: {e}");
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

    let t0 = Instant::now();
    mainloop.run();
    let dur = t0.elapsed().as_secs_f64();

    let g = p.borrow();
    let pct = |v: &Vec<f64>, q: usize| {
        let mut s = v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s.get(s.len() * q / 100).copied().unwrap_or(0.0)
    };
    println!("\n  {} quadros {}x{} em {dur:.2}s = {:.1} fps",
        g.n, g.w, g.h, g.n as f64 / dur.max(0.001));
    println!("  copia:  p50 {:.2}ms  p95 {:.2}ms", pct(&g.copia_ms, 50), pct(&g.copia_ms, 95));
    println!("  encode: p50 {:.2}ms  p95 {:.2}ms", pct(&g.enc_ms, 50), pct(&g.enc_ms, 95));
    println!("  total:  p50 {:.2}ms  p95 {:.2}ms  (orcamento 16.6ms a 60fps)",
        pct(&g.total_ms, 50), pct(&g.total_ms, 95));
    println!("  {:.1} MB = {:.1} Mbps a 60fps",
        g.bytes as f64 / 1_048_576.0,
        (g.bytes as f64 * 8.0 / g.n.max(1) as f64) * 60.0 / 1_000_000.0);
    println!("  {}", destino.display());
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    Ok(())
}
