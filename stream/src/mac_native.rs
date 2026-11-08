//! The native capture-and-encode path on macOS: ScreenCaptureKit into
//! VideoToolbox, with the `CVPixelBuffer` going straight from one to the
//! other with no conversion.
//!
//! Spec: ../../specs/020-native-mac.md
//!
//! It lives in the library and not in a probe binary for one reason:
//! **macOS ties the Screen Recording grant to the binary**, to its
//! designated requirement. A fresh probe has no grant and gets
//! `-3801 The user declined TCCs`, measured. `oxdesk-host` already has it,
//! and keeps it across rebuilds because the local signing identity keeps
//! the requirement stable -- which is exactly what that identity is for.
//! So the host calls this, and the standalone probe stays for a machine
//! where someone is present to approve it.
#![cfg(target_os = "macos")]

pub fn probe() -> Result<(), String> {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSError};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
    };
    use crate::vt;

    println!("== ScreenCaptureKit -> VideoToolbox, do Rust, sem nada instalado ==");

    // ---- 1. what can be captured. The completion handler runs on another
    // ---- thread, so the channel is both the wait and the hand-off.
    let (tx, rx) = mpsc::channel::<Result<Retained<SCShareableContent>, String>>();
    let bloco = RcBlock::new(move |conteudo: *mut SCShareableContent, erro: *mut NSError| {
        let r = if !conteudo.is_null() {
            match unsafe { Retained::retain(conteudo) } {
                Some(c) => Ok(c),
                None => Err("SCShareableContent veio nulo depois do retain".to_string()),
            }
        } else if !erro.is_null() {
            let e = unsafe { &*erro };
            Err(format!("{} (codigo {})", e.localizedDescription(), e.code()))
        } else {
            Err("sem conteudo e sem erro".to_string())
        };
        let _ = tx.send(r);
    });
    let t0 = Instant::now();
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&bloco) };

    let conteudo = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(format!(
                "ScreenCaptureKit recusou: {e}\n  \
                 A Gravacao de Tela pertence ao BINARIO. Se este e novo, aprove-o em \
                 Ajustes > Privacidade e Seguranca > Gravacao de Tela. Ver specs/019."
            ));
        }
        Err(_) => {
            return Err("o handler de SCShareableContent nao respondeu em 15s".into());
        }
    };
    println!("  SCShareableContent: respondeu em {} ms", t0.elapsed().as_millis());

    let telas = unsafe { conteudo.displays() };
    if telas.is_empty() {
        return Err("nenhuma tela compartilhavel -- quase sempre falta de permissao".into());
    }
    let tela = telas.firstObject().expect("a lista nao esta vazia");
    let (w, h) = unsafe { (tela.width(), tela.height()) };
    println!("  tela: {w}x{h}, {} tela(s) disponivel(is)", telas.len());

    // ---- 2. filter and configuration. BGRA because that is what the
    // ---- encoder takes without a conversion.
    let vazio = NSArray::<SCWindow>::new();
    let filtro = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &tela, &vazio)
    };
    let config = unsafe { SCStreamConfiguration::init(SCStreamConfiguration::alloc()) };
    unsafe {
        // `SCDisplay` answers isize, the configuration takes usize.
        config.setWidth(w as usize);
        config.setHeight(h as usize);
        config.setPixelFormat(vt::PIXEL_BGRA);
        // The cursor is drawn by the compositor here, like Mutter does and
        // unlike DXGI -- which is why Windows needs an overlay (specs/010).
        config.setShowsCursor(true);
    }
    println!("  configuracao: BGRA, cursor desenhado pelo compositor");

    // ---- 3. one frame.
    let (tx2, rx2) = mpsc::channel::<Result<usize, String>>();
    let bloco2 = RcBlock::new(
        move |amostra: *mut objc2_core_media::CMSampleBuffer, erro: *mut NSError| {
            if amostra.is_null() {
                let msg = if erro.is_null() {
                    "sem amostra e sem erro".to_string()
                } else {
                    let e = unsafe { &*erro };
                    format!("{}", e.localizedDescription())
                };
                let _ = tx2.send(Err(msg));
                return;
            }
            // The pointer is only valid inside the handler, so the frame is
            // encoded here rather than sent across the channel.
            let _ = tx2.send(encode_aqui(amostra as *mut std::ffi::c_void));
        },
    );
    let t1 = Instant::now();
    unsafe {
        SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(
            &filtro,
            &config,
            Some(&bloco2),
        )
    };
    match rx2.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(bytes)) => {
            println!("  captura + encode: {} ms, {bytes} bytes de H.264", t1.elapsed().as_millis());
            println!();
            println!("  => cadeia nativa completa: captura e encode sem FFmpeg e sem Homebrew");
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("a captura nao respondeu em 20s".into()),
    }
    Ok(())
}

/// Encodes the frame while the sample buffer is still valid, and prints what
/// came out. Returns the access unit's size.
fn encode_aqui(amostra: *mut std::ffi::c_void) -> Result<usize, String> {
    use crate::vt;
    let pixels = unsafe { vt::CMSampleBufferGetImageBuffer(amostra) };
    if pixels.is_null() {
        return Err("a amostra nao tem CVPixelBuffer".into());
    }
    let (w, h) = unsafe {
        (vt::CVPixelBufferGetWidth(pixels) as i32, vt::CVPixelBufferGetHeight(pixels) as i32)
    };
    println!("  CVPixelBuffer da captura: {w}x{h} (sem conversao, vai direto pro encoder)");
    let mut enc = vt::Encoder::new(w, h)?;
    let q = enc.encode(pixels, vt::CMTime::new(0, 60), vt::CMTime::new(1, 60))?;
    for (i, p) in q.parametros.iter().enumerate() {
        let nal = p.first().map(|b| b & 0x1f).unwrap_or(0);
        println!(
            "  parameter set {i}: {} bytes, nal {nal} ({})",
            p.len(),
            match nal {
                7 => "SPS",
                8 => "PPS",
                _ => "?",
            }
        );
    }
    let b = &q.bytes;
    let mut i = 0usize;
    let mut n = 0;
    while i + 4 <= b.len() && n < 4 {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        if len == 0 || i + 4 + len > b.len() {
            break;
        }
        let nal = b[i + 4] & 0x1f;
        println!(
            "  nal {n}: tipo {nal} ({}), {len} bytes",
            match nal {
                1 => "non-IDR slice",
                5 => "IDR slice",
                6 => "SEI",
                _ => "?",
            }
        );
        i += 4 + len;
        n += 1;
    }
    Ok(b.len())
}
