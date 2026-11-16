// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Host on macOS: avfoundation -> VideoToolbox -> QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why through ffmpeg, and not raw ScreenCaptureKit
//!
//! Measured on the machine (see `oxdesk-probe-mac`): brew's ffmpeg has
//! `avfoundation` as an input device and `h264_videotoolbox` as a hardware
//! encoder. Together they give the whole pipeline with no new FFI -- and
//! the library is already installed because it is what decodes in that
//! machine's viewer.
//!
//! Raw ScreenCaptureKit and VideoToolbox would be hundreds of lines of
//! Objective-C bridging to get to the same place, with the sole advantage
//! of not needing brew. Not worth it.
//!
//! ## Visible difference from the other two hosts
//!
//! `avfoundation` can draw the cursor (`capture_cursor`), like Mutter and
//! unlike DXGI. So the Mac's screen goes out with the cursor embedded, like
//! the Linux one.

#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::serve::{self, now_ns, EncodedFrame, Signals};

/// Two frames of slack. More than that would be a queue of stale stuff.
const CAPACITY: usize = 2;

/// Target cadence: 60 Hz, the same ceiling as the Linux host and for the
/// same reason -- above the viewer's refresh rate it does not become
/// picture, it becomes bandwidth and color-conversion work.
const CADENCE: Duration = Duration::from_micros(16_667);

/// The main screen, by the name `avfoundation` gives it.
const SCREEN: &str = "Capture screen 0";

/// The device to open: an index becomes the screen name.
///
/// The index counts cameras, so it changes on its own. With an iPhone
/// nearby the screen went from `1` to `3`, and `1` became the iPhone's
/// camera: the host served that camera, as a black frame, without any
/// error. Old installs still pass `1`, which is why it is handled here.
fn device(requested: &str) -> &str {
    if requested.parse::<u32>().is_ok() {
        SCREEN
    } else {
        requested
    }
}

/// Opens the `avfoundation` screen capture.
///
/// The index comes as text because that is what the device URL is: in
/// avfoundation screen 0 is `"1"` when there is a camera ahead of it, and
/// the only honest way to know is to list -- so the index is configurable
/// and the default is what works on this machine.
fn open_screen(index: &str, fps: u32) -> anyhow::Result<ffmpeg_next::format::context::Input> {
    let name = std::ffi::CString::new("avfoundation")?;
    // `format::list()` existed and was removed in ffmpeg 5. Looking it up
    // by name is the current way, and it is what `ffmpeg -f` itself does.
    let raw = unsafe { ffmpeg_next::ffi::av_find_input_format(name.as_ptr()) };
    if raw.is_null() {
        anyhow::bail!("avfoundation is not in this ffmpeg");
    }
    let input = unsafe { ffmpeg_next::format::format::Input::wrap(raw as *mut _) };
    let fmt = ffmpeg_next::format::format::Format::Input(input);

    let mut opts = ffmpeg_next::Dictionary::new();
    opts.set("framerate", &fps.to_string());
    // Cursor in the picture: without it the viewer cannot see where it
    // points. DXGI does not offer this option; this one does, so we use it.
    opts.set("capture_cursor", "1");
    // No drawn clicks: a white circle on every click is a distraction.
    opts.set("capture_mouse_clicks", "0");
    // NO `pixel_format`: avfoundation's screen capture rejects the option
    // and falls back to the default warning `Configuration of video device
    // failed, falling back to default` -- measured. Whatever format it
    // delivers is converted afterwards by `swscale`, which is in the path
    // anyway.

    let ctx = ffmpeg_next::format::open_with(&index, &fmt, opts)
        .map_err(|e| anyhow::anyhow!("open screen {index}: {e}"))?;
    match ctx {
        ffmpeg_next::format::context::Context::Input(i) => Ok(i),
        _ => anyhow::bail!("avfoundation returned an output context"),
    }
}

pub fn run(port: u16, index: &str) -> anyhow::Result<()> {
    let index = device(index);
    ffmpeg_next::init()?;
    // Without this the name `avfoundation` does not resolve: input devices
    // live in libavdevice and have to be registered.
    ffmpeg_next::device::register_all();

    // Capture hangs, it does not fail, when the Screen Recording permission
    // is missing -- and the process sits in `S` without a single error.
    // Warn BEFORE trying: if we hang, at least the last line of the log
    // says where to look.
    println!("  opening screen {index}... (if it stops here, this binary lacks Screen Recording)");
    let mut input = open_screen(index, 60)?;
    let stream = input
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("the capture has no video stream"))?;
    let stream_index = stream.index();
    let mut dec = ffmpeg_next::codec::context::Context::from_parameters(stream.parameters())?
        .decoder()
        .video()?;
    let (w, h) = (dec.width(), dec.height());
    println!("  avfoundation: screen {index}, {w}x{h}, {:?}", dec.format());

    let codec = ffmpeg_next::encoder::find_by_name("h264_videotoolbox")
        .ok_or_else(|| anyhow::anyhow!("no h264_videotoolbox"))?;
    let mut enc = ffmpeg_next::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    enc.set_width(w);
    enc.set_height(h);
    // NV12 is VideoToolbox's native format; asking for another one would
    // make it convert internally, without telling us the cost.
    enc.set_format(ffmpeg_next::format::Pixel::NV12);
    enc.set_time_base(ffmpeg_next::Rational::new(1, 60));
    enc.set_frame_rate(Some(ffmpeg_next::Rational::new(60, 1)));
    // Same scale as the other two hosts (see `nvenc::NvencEncoder::new`):
    // 4 bits per pixel per second at 60 fps.
    // Same formula as NVENC, from the same place: this was the SECOND copy.
    let target = crate::quality::target(w, h) as u64;
    enc.set_bit_rate(target as usize);
    enc.set_max_bit_rate((target * 2) as usize);
    // IDR every 2s: lose a frame and the error heals in 2s instead of
    // smearing the picture forever.
    enc.set_gop(120);
    // No B frames: they improve compression at the cost of delaying
    // output, and here latency matters more than bitrate.
    enc.set_max_b_frames(0);
    let mut opts = ffmpeg_next::Dictionary::new();
    // `realtime`: VideoToolbox stops accumulating frames to decide better,
    // which is the opposite of what a remote screen wants.
    opts.set("realtime", "true");
    let mut enc = enc.open_with(opts)?;
    println!("  h264_videotoolbox ready, {} Mbps target", target / 1_000_000);

    // Converter to NV12, created only if the capture format differs.
    //
    // NV12 cannot be requested from the capture (the option is rejected),
    // and feeding VideoToolbox another format would make it convert
    // internally, without telling us the cost. Converting here, the cost is
    // visible and sits somewhere it can be measured.
    let mut scaler = if dec.format() == ffmpeg_next::format::Pixel::NV12 {
        None
    } else {
        println!("  converting {:?} -> NV12", dec.format());
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

    let (tx, rx) = tokio::sync::mpsc::channel::<EncodedFrame>(CAPACITY);
    let signals = Arc::new(Signals::default());
    let origin = Arc::new(Mutex::new((0i32, 0i32)));

    let network = {
        let signals = Arc::clone(&signals);
        let origin = Arc::clone(&origin);
        std::thread::spawn(move || {
            if let Err(e) = run_network(port, rx, Arc::clone(&signals), origin) {
                eprintln!("  network: {e}");
            }
            signals.shutdown();
        })
    };

    println!("  host on port {port}; ctrl-c to stop");
    // Warn NOW, not when input does not work.
    //
    // Without the Accessibility permission `CGEventPost` accepts the event
    // and silently drops it -- returns success and nothing happens. It is
    // the same failure mode as Screen Recording, and it is what made the
    // user spend an afternoon thinking the KVM was broken.
    if crate::input_mac::has_accessibility() {
        println!("  accessibility granted: keyboard and mouse will be injected");
        // Logged because "the cursor position did not match" is a symptom
        // that cannot be diagnosed without this number: it says in which
        // space the pointer is injected versus the space the screen is
        // captured in.
        let e = crate::input_mac::display_scale();
        println!("  {e:.0} capture pixel(s) per pointer point");
    } else {
        println!(
            "  WARNING: NO Accessibility permission. Incoming keyboard and mouse \
             will be silently DROPPED by macOS. Grant it in System Settings > \
             Privacy & Security > Accessibility, for oxdesk-host."
        );
    }
    let mut next_due = Instant::now();
    let mut n = 0i64;
    let mut dropped = 0u64;

    // `Packet::read` directly, and NOT the `packets()` iterator.
    //
    // The iterator ends on any error that is not end of file -- including
    // `EAGAIN`, which is the normal answer from a capture device that has
    // no frame ready yet. The crate's own documentation says so: "callers
    // that must observe these errors drive `Packet::read` directly".
    //
    // The symptom was exact: the host opened the screen, built the encoder,
    // announced the port and printed "capture ended" **before** anyone
    // connected. The user's log had the clue one line above: `not enough
    // frames to estimate rate`. The device had just opened and had no
    // frame; the first read returned "try again"; the iterator read that as
    // the end of the stream.
    let mut packet = ffmpeg_next::Packet::empty();
    loop {
        if signals.should_exit() {
            break;
        }
        match packet.read(&mut input) {
            Ok(()) => {}
            // A real end: the device went away.
            Err(ffmpeg_next::Error::Eof) => break,
            // Anything else is "not yet": capture does not end, it waits.
            // Sleeping a little avoids spinning at 100% CPU.
            Err(_) => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
        }
        if packet.stream() != stream_index {
            continue;
        }
        if dec.send_packet(&packet).is_err() {
            continue;
        }
        let mut frame = ffmpeg_next::frame::Video::empty();
        while dec.receive_frame(&mut frame).is_ok() {
            // With nobody watching, do not encode: the frame would go into
            // a channel nobody drains.
            if !signals.has_client() {
                continue;
            }
            let now = Instant::now();
            if now < next_due {
                continue;
            }
            next_due = (next_due + CADENCE).max(now);

            // Convert when needed. `frame` below is what goes to the
            // encoder: the original when it is already right, the
            // converted one when not.
            let mut converted = ffmpeg_next::frame::Video::empty();
            let frame = match scaler.as_mut() {
                Some(sc) => {
                    if sc.run(&frame, &mut converted).is_err() {
                        continue;
                    }
                    &mut converted
                }
                None => &mut frame,
            };

            let captured = now_ns();
            if signals.take_idr() {
                // Keyframe request: the new client has no context.
                frame.set_kind(ffmpeg_next::picture::Type::I);
            } else {
                frame.set_kind(ffmpeg_next::picture::Type::None);
            }
            frame.set_pts(Some(n));
            n += 1;
            if enc.send_frame(&frame).is_err() {
                continue;
            }
            let mut out = ffmpeg_next::Packet::empty();
            while enc.receive_packet(&mut out).is_ok() {
                let Some(data) = out.data() else { continue };
                let keyframe = out.is_key();
                if tx
                    .try_send(EncodedFrame {
                        data: data.to_vec(),
                        keyframe,
                        captured_at_ns: captured,
                    })
                    .is_err()
                {
                    dropped += 1;
                    if dropped % 120 == 0 {
                        println!("  {dropped} frames dropped (network slower than the GPU)");
                    }
                }
            }
        }
    }

    println!("  capture ended");
    let _ = network.join();
    Ok(())
}

/// Network: video by datagram, input by stream.
///
/// Plain `tokio::spawn`, like on Windows: `CGEvent` has no owning thread.
/// Unlike Linux, where the Mutter session belongs to the D-Bus connection
/// that created it.
fn run_network(
    port: u16,
    mut rx: tokio::sync::mpsc::Receiver<EncodedFrame>,
    signals: Arc<Signals>,
    _origin: Arc<Mutex<(i32, i32)>>,
) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        let listen = crate::bind::listen_addr();
        let ep = crate::net::server((listen, port).into())?;
        // The REAL address, not a hardcoded "0.0.0.0": with --bind overlay
        // the host listens somewhere else, and a log saying 0.0.0.0 would
        // send whoever is diagnosing to look in the wrong place.
        println!("  listening on {listen}:{port}");

        // Input port, next to the video one.
        //
        // It was missing here: the Windows and Linux hosts already had it,
        // and the Mac one did not -- so the KVM found nowhere to send and
        // timed out. Without it, watching and controlling fight over the
        // single video-client slot.
        let input_port = port + 1;
        let ep_in = crate::net::server((listen, input_port).into())?;
        println!("  input on {listen}:{input_port}");
        {
            let signals_in = Arc::clone(&signals);
            tokio::spawn(async move {
                serve::serve_input(&ep_in, Arc::clone(&signals_in), |conn| {
                    let conn = conn.clone();
                    tokio::spawn(async move { receive_input(conn).await })
                })
                .await;
            });
        }

        serve::serve_video(&ep, &mut rx, &signals, |conn| {
            let conn = conn.clone();
            tokio::spawn(async move { receive_input(conn).await })
        })
        .await
    })
}

/// Reads a connection's input stream and injects until it closes.
///
/// One for both ports: the viewer sends input over the video connection,
/// the KVM sends it over the input port, and both deserve the same
/// treatment.
async fn receive_input(conn: quinn::Connection) {
    let Ok(mut recv) = conn.accept_uni().await else { return };
    // One channel per connection: the thread dies when the client leaves,
    // and a new `Injector` starts with the pointer's real position.
    let keyboard = crate::input_mac::thread();
    let mut reader = crate::input::Reader::default();
    let mut buf = vec![0u8; 4096];
    let mut n = 0u64;
    let mut held = crate::input::Held::default();
    while let Ok(Some(read)) = recv.read(&mut buf).await {
        reader.feed(&buf[..read]);
        while let Some(e) = reader.next_event() {
            held.note(&e);
            if keyboard.send(e).is_err() {
                return; // the injection thread exited
            }
            n += 1;
            // The first ones, one by one: "no error" is weak proof that the
            // input arrived -- and on macOS it is NO proof at all, because
            // without Accessibility CGEventPost returns success and drops.
            if n <= 8 {
                println!("  input #{n}: {e:?}");
            } else if n % 500 == 0 {
                println!("  {n} input events applied");
            }
        }
    }
    // The stream ended -- cleanly or not. Release what the client left
    // held (`input::Held`).
    let left = held.release_all();
    if !left.is_empty() {
        for e in &left {
            let _ = keyboard.send(*e);
        }
        println!("  input: released {} key(s)/button(s) the client left held", left.len());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn index_never_opens_camera() {
        assert_eq!(super::device("1"), super::SCREEN);
        assert_eq!(super::device("3"), super::SCREEN);
        assert_eq!(super::device("Capture screen 1"), "Capture screen 1");
    }
}
