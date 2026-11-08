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

    use crate::vt;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSError};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
    };

    println!("== ScreenCaptureKit -> VideoToolbox, from Rust, nothing installed ==");

    // ---- 1. what can be captured. The completion handler runs on another
    // ---- thread, so the channel is both the wait and the hand-off.
    let (tx, rx) = mpsc::channel::<Result<Retained<SCShareableContent>, String>>();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        let r = if !content.is_null() {
            match unsafe { Retained::retain(content) } {
                Some(c) => Ok(c),
                None => Err("SCShareableContent came back null after retain".to_string()),
            }
        } else if !error.is_null() {
            let e = unsafe { &*error };
            Err(format!("{} (code {})", e.localizedDescription(), e.code()))
        } else {
            Err("no content and no error".to_string())
        };
        let _ = tx.send(r);
    });
    let t0 = Instant::now();
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };

    let content = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            return Err(format!(
                "ScreenCaptureKit refused: {e}\n  \
                 Screen Recording belongs to the BINARY. If this one is new, approve it in \
                 Settings > Privacy & Security > Screen Recording. See specs/019."
            ));
        }
        Err(_) => {
            return Err("the SCShareableContent handler did not answer within 15s".into());
        }
    };
    println!("  SCShareableContent: answered in {} ms", t0.elapsed().as_millis());

    let displays = unsafe { content.displays() };
    if displays.is_empty() {
        return Err("no shareable display -- almost always a missing permission".into());
    }
    let display = displays.firstObject().expect("the list is not empty");
    let (w, h) = unsafe { (display.width(), display.height()) };
    println!("  display: {w}x{h}, {} display(s) available", displays.len());

    // ---- 2. filter and configuration. BGRA because that is what the
    // ---- encoder takes without a conversion.
    let no_windows = NSArray::<SCWindow>::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &no_windows,
        )
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
    println!("  configuration: BGRA, cursor drawn by the compositor");

    // ---- 3. one frame.
    let (tx2, rx2) = mpsc::channel::<Result<usize, String>>();
    let frame_handler = RcBlock::new(
        move |sample: *mut objc2_core_media::CMSampleBuffer, error: *mut NSError| {
            if sample.is_null() {
                let msg = if error.is_null() {
                    "no sample and no error".to_string()
                } else {
                    let e = unsafe { &*error };
                    format!("{}", e.localizedDescription())
                };
                let _ = tx2.send(Err(msg));
                return;
            }
            // The pointer is only valid inside the handler, so the frame is
            // encoded here rather than sent across the channel.
            let _ = tx2.send(encode_in_place(sample as *mut std::ffi::c_void));
        },
    );
    let t1 = Instant::now();
    unsafe {
        SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(
            &filter,
            &config,
            Some(&frame_handler),
        )
    };
    match rx2.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(bytes)) => {
            println!(
                "  capture + encode: {} ms, {bytes} bytes of H.264",
                t1.elapsed().as_millis()
            );
            println!();
            println!("  => the whole native chain: capture and encode with no FFmpeg and no Homebrew");
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err("the capture did not answer within 20s".into()),
    }
    Ok(())
}

/// Encodes the frame while the sample buffer is still valid, and prints what
/// came out. Returns the access unit's size.
fn encode_in_place(sample: *mut std::ffi::c_void) -> Result<usize, String> {
    use crate::vt;
    let pixels = unsafe { vt::CMSampleBufferGetImageBuffer(sample) };
    if pixels.is_null() {
        return Err("the sample carries no CVPixelBuffer".into());
    }
    let (w, h) = unsafe {
        (vt::CVPixelBufferGetWidth(pixels) as i32, vt::CVPixelBufferGetHeight(pixels) as i32)
    };
    println!("  CVPixelBuffer from the capture: {w}x{h} (no conversion, straight to the encoder)");
    let mut enc = vt::Encoder::new(w, h)?;
    let frame = enc.encode(pixels, vt::CMTime::new(0, 60), vt::CMTime::new(1, 60))?;
    for (i, p) in frame.parameter_sets.iter().enumerate() {
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
    let b = &frame.bytes;
    let mut i = 0usize;
    let mut n = 0;
    while i + 4 <= b.len() && n < 4 {
        let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        if len == 0 || i + 4 + len > b.len() {
            break;
        }
        let nal = b[i + 4] & 0x1f;
        println!(
            "  nal {n}: type {nal} ({}), {len} bytes",
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
