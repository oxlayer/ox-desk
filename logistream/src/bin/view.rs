//! Viewer: QUIC -> H.264 -> window. Closes out v2.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Where each thing runs
//!
//! Network and decoding on one thread, drawing on the UI thread. The
//! window loop owns the main thread (a macOS requirement, good practice
//! elsewhere) and must not block -- if it decoded there, every frame would
//! freeze the UI.
//!
//! ## Where it compiles
//!
//! On all three. Who decodes changes per platform and lives in
//! `logistream::decode`: NVDEC on Linux and Windows (it ships with the
//! NVIDIA driver), `ffmpeg` on the Mac (which has no NVIDIA). NVDEC is what
//! took Windows off RDP -- there is no `libavcodec` there and shipping the
//! DLLs would be plumbing with no gain.
//!
//! Between the two there is **a slot**, not a queue: a new frame
//! overwrites one not yet drawn. A queue would only pile up delay, and on
//! a remote screen an old frame has no value -- it is already past.
//!
//!   cargo run --release --bin view -- <host:port>

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn main() {
    eprintln!(
        "the viewer does not build on this system: it needs NVDEC \
         (Linux/Windows with an NVIDIA GPU) or libavcodec (macOS). \
         See specs/010."
    );
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn main() -> anyhow::Result<()> {
    visual::run()
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
mod visual {
    use std::num::NonZeroU32;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use logistream::input::{evdev_from_winit, Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
    use logistream::net::{self, Reassembler};
    use winit::event::{ElementState, MouseButton, MouseScrollDelta};
    use winit::keyboard::PhysicalKey;
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
    use winit::window::{Window, WindowId};

    /// The frame ready to draw, with the geometry that explains it.
    ///
    /// Three different sizes live here, and mixing them up was a visible
    /// bug: the REMOTE screen (`rw`x`rh`), the drawn IMAGE (`w`x`h`, the
    /// remote one fitted to the window without distortion) and the WINDOW
    /// (`win_w`x`win_h`).
    ///
    /// The mouse position math used the image size as if it were the
    /// remote screen's. Since the image is the size of the window,
    /// `position / window * image` returned the position itself -- in a
    /// 1280x720 window against a 2560x1440 screen, the cursor only reached
    /// the top-left QUARTER of the remote desktop. That is what the user
    /// saw as "the position doesn't match".
    #[derive(Default)]
    pub struct Slot {
        pub rgb: Vec<u32>,
        /// Size of the drawn image.
        pub w: u32,
        pub h: u32,
        /// Corner where the image starts inside the window (side bars).
        pub ox: u32,
        pub oy: u32,
        /// Remote screen size -- the host's coordinate space.
        pub rw: u32,
        pub rh: u32,
        /// Window this geometry was computed for.
        pub win_w: u32,
        pub win_h: u32,
        /// Goes up on every new frame; the window only redraws when it changes.
        pub generation: u64,
        /// Capture instant of the frame in the slot, for `LOGI_MEDIR`.
        pub captured_ns: u64,
    }

    /// Color of the bars when the window's aspect ratio does not match the
    /// remote screen's. Dark and neutral: a light bar pulls the eye away
    /// from the image.
    const BACKGROUND: u32 = 0x0010_1014;

    /// Image fitted to the window WITHOUT distortion: scale by the tighter
    /// side, center the rest.
    ///
    /// Before, the image was stretched to the exact window size. On a
    /// 16:10 screen showing a 16:9 desktop that deforms everything -- and,
    /// worse, a circle on the host is not a circle here, which gets in the
    /// way of exactly the person trying to work on the other machine.
    fn fit(rw: u32, rh: u32, win_w: u32, win_h: u32) -> (u32, u32, u32, u32) {
        if rw == 0 || rh == 0 || win_w == 0 || win_h == 0 {
            return (0, 0, 0, 0);
        }
        let e = (win_w as f64 / rw as f64).min(win_h as f64 / rh as f64);
        let w = ((rw as f64 * e).round() as u32).clamp(1, win_w);
        let h = ((rh as f64 * e).round() as u32).clamp(1, win_h);
        ((win_w - w) / 2, (win_h - h) / 2, w, h)
    }

    /// Position in the window -> position on the remote screen.
    ///
    /// `None` when the cursor is over the bar and not over the image:
    /// there is no matching remote point, and clamping to the edge would
    /// invent a movement the person did not make.
    ///
    /// Pure and separate from the event loop because this is exactly where
    /// the bug lived -- and a scale error raises no error, it just puts the
    /// cursor in the wrong place, which you only find out by using it.
    #[allow(clippy::type_complexity)]
    fn remote_point(
        px: f64,
        py: f64,
        (ox, oy, iw, ih, rw, rh): (f64, f64, f64, f64, f64, f64),
    ) -> Option<(f64, f64)> {
        if iw <= 0.0 || ih <= 0.0 {
            return None;
        }
        let x = (px - ox) / iw * rw;
        let y = (py - oy) / ih * rh;
        if x < 0.0 || y < 0.0 || x >= rw || y >= rh {
            return None;
        }
        Some((x, y))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn same_aspect_fills_without_bars() {
            // 2560x1440 in a 1280x720 window: fits exactly, no bar.
            assert_eq!(fit(2560, 1440, 1280, 720), (0, 0, 1280, 720));
        }

        #[test]
        fn wider_window_gets_side_bars() {
            // 16:9 in a 2:1 window -> limited by height, width left over.
            // 1920 * (1000/1080) = 1777.8 -> 1778, rounded and not
            // truncated: truncating shrinks the image one pixel on every
            // resize and the cursor math inherits the error.
            let (ox, oy, w, h) = fit(1920, 1080, 2000, 1000);
            assert_eq!((w, h), (1778, 1000));
            assert_eq!(oy, 0, "no bar on top when limited by height");
            assert_eq!(ox, (2000 - 1778) / 2, "centered");
        }

        #[test]
        fn taller_window_gets_bars_above() {
            let (ox, oy, w, h) = fit(1920, 1080, 1000, 1000);
            assert_eq!((w, h), (1000, 563));
            assert_eq!(ox, 0);
            assert_eq!(oy, (1000 - 563) / 2);
        }

        #[test]
        fn the_image_never_exceeds_the_window() {
            // Rounding up must not create an image larger than the
            // window: drawing would clip and the cursor math would lie.
            for (rw, rh, win_w, win_h) in
                [(2560u32, 1440u32, 1281u32, 721u32), (1366, 768, 999, 1), (3, 7, 100, 100)]
            {
                let (ox, oy, w, h) = fit(rw, rh, win_w, win_h);
                assert!(w <= win_w && h <= win_h, "{rw}x{rh} in {win_w}x{win_h} gave {w}x{h}");
                assert!(ox + w <= win_w && oy + h <= win_h);
            }
        }

        #[test]
        fn zero_size_does_not_divide_by_zero() {
            assert_eq!(fit(0, 0, 100, 100), (0, 0, 0, 0));
            assert_eq!(fit(100, 100, 0, 0), (0, 0, 0, 0));
            assert_eq!(remote_point(1.0, 1.0, (0.0, 0.0, 0.0, 0.0, 10.0, 10.0)), None);
        }

        #[test]
        fn the_cursor_covers_the_whole_remote_screen() {
            // The bug: in a 1280x720 window against 2560x1440, the cursor
            // only reached the top-left quarter. The bottom-right corner
            // of the window has to reach the corner of the remote screen.
            let g = (0.0, 0.0, 1280.0, 720.0, 2560.0, 1440.0);
            assert_eq!(remote_point(0.0, 0.0, g), Some((0.0, 0.0)));
            assert_eq!(remote_point(640.0, 360.0, g), Some((1280.0, 720.0)));
            let (x, y) = remote_point(1279.0, 719.0, g).expect("inside");
            assert!(x > 2550.0 && y > 1430.0, "corner became {x},{y}");
        }

        #[test]
        fn over_the_bar_sends_nothing() {
            // 800x600 image centered in a 1000x600 window: a 100px bar
            // on each side.
            let g = (100.0, 0.0, 800.0, 600.0, 1920.0, 1440.0);
            assert_eq!(remote_point(50.0, 300.0, g), None, "left bar");
            assert_eq!(remote_point(950.0, 300.0, g), None, "right bar");
            assert!(remote_point(100.0, 300.0, g).is_some(), "first column of the image");
            // And the start of the image is the start of the remote
            // screen, not a 100px offset.
            assert_eq!(remote_point(100.0, 0.0, g), Some((0.0, 0.0)));
        }
    }

    pub fn run() -> anyhow::Result<()> {
        let target = std::env::args().nth(1).unwrap_or_else(|| "ubuntu-host.local:9777".into());
        let slot: Arc<Mutex<Slot>> = Arc::new(Mutex::new(Slot::default()));
        let slot_net = Arc::clone(&slot);

        // A tokio channel used from SYNCHRONOUS code (the window loop):
        // `try_send` does not block, and if it fills up it is because
        // input is arriving faster than the network -- dropping mouse
        // movement is acceptable, and key events are rare enough to never
        // fill it.
        let (tx_in, rx_in) = tokio::sync::mpsc::channel::<Event>(256);

        // The WINDOW size, shared with whoever scales.
        //
        // I sized the surface by the FRAME size: in a 1280x720 window
        // showing a 2560x1440 stream, the buffer does not match the window
        // and the result is undefined -- that is what the user saw as
        // "glitched image". `swscale` does the resizing now; it was
        // already on the path anyway.
        let size: Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((1280, 720)));
        let size_net = Arc::clone(&size);

        // The window loop has to be created BEFORE the network thread: the
        // `proxy` comes from it, and it is through the proxy that the
        // network wakes the UI when a frame arrives.
        let ev = EventLoop::new()?;
        let proxy = ev.create_proxy();

        std::thread::spawn(move || {
            if let Err(e) = receive(&target, slot_net, rx_in, size_net, proxy) {
                eprintln!("network: {e}");
                std::process::exit(1);
            }
        });

        // `Wait` and not `Poll`: we wake when a frame arrives, not at 100%
        // CPU asking whether one arrived.
        ev.set_control_flow(ControlFlow::Wait);
        let mut app = App {
            window: None,
            surface: None,
            slot,
            drawn: 0,
            n: 0,
            t0: Instant::now(),
            tx_in,
            window_size: (1280.0, 720.0),
            size,
            measure: std::env::var_os("LOGI_MEDIR").is_some(),
            delays: Vec::new(),
        };
        ev.run_app(&mut app)?;
        Ok(())
    }

    struct App {
        window: Option<Arc<Window>>,
        surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
        slot: Arc<Mutex<Slot>>,
        drawn: u64,
        n: u64,
        t0: Instant,
        tx_in: tokio::sync::mpsc::Sender<Event>,
        /// Current window size, to convert the cursor position into the
        /// stream's space.
        window_size: (f64, f64),
        /// Same size, in physical pixels, for the scaling thread.
        size: Arc<Mutex<(u32, u32)>>,
        /// `LOGI_MEDIR=1`: prints the delay from capture to screen.
        measure: bool,
        delays: Vec<f64>,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, ev: &ActiveEventLoop) {
            let attrs = Window::default_attributes()
                .with_title("logistream")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
            let window = Arc::new(ev.create_window(attrs).expect("window"));
            let ctx = softbuffer::Context::new(window.clone()).expect("context");
            let surface = softbuffer::Surface::new(&ctx, window.clone()).expect("surface");
            self.window = Some(window);
            self.surface = Some(surface);
        }

        fn window_event(&mut self, ev: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => ev.exit(),
                WindowEvent::RedrawRequested => self.draw(),
                WindowEvent::Resized(t) => {
                    self.window_size = (t.width as f64, t.height as f64);
                    if let Ok(mut g) = self.size.lock() {
                        *g = (t.width.max(1), t.height.max(1));
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    // The position goes in REMOTE SCREEN coordinates -- the
                    // host defines the space, not us. And the conversion is
                    // through the drawn IMAGE, not the window: with side
                    // bars the two differ, and using the window would throw
                    // the remote cursor off by the whole width of the bar.
                    let (rw, rh, ox, oy, iw, ih) = match self.slot.lock() {
                        Ok(s) if s.rw > 0 && s.w > 0 => {
                            (s.rw as f64, s.rh as f64, s.ox as f64, s.oy as f64,
                             s.w as f64, s.h as f64)
                        }
                        _ => return,
                    };
                    let Some((x, y)) =
                        remote_point(position.x, position.y, (ox, oy, iw, ih, rw, rh))
                    else {
                        return;
                    };
                    self.send(Event::Mouse { x, y });
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let code = match button {
                        MouseButton::Left => BTN_LEFT,
                        MouseButton::Right => BTN_RIGHT,
                        MouseButton::Middle => BTN_MIDDLE,
                        _ => return,
                    };
                    self.send(Event::Button {
                        code,
                        pressed: state == ElementState::Pressed,
                    });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    // The Mac trackpad sends pixels; the wheel sends
                    // lines. Mutter wants a continuous offset, so a line
                    // becomes a step of ~15 px.
                    let (dx, dy) = match delta {
                        MouseScrollDelta::LineDelta(x, y) => (-x as f64 * 15.0, -y as f64 * 15.0),
                        MouseScrollDelta::PixelDelta(p) => (-p.x, -p.y),
                    };
                    self.send(Event::Scroll { dx, dy });
                }
                WindowEvent::KeyboardInput { event, is_synthetic, .. } => {
                    // Synthetic is what the system makes up when the
                    // window gets focus; forwarding it would press a key
                    // nobody pressed.
                    if is_synthetic {
                        return;
                    }
                    let PhysicalKey::Code(code) = event.physical_key else { return };
                    let Some(keycode) = evdev_from_winit(code) else { return };
                    self.send(Event::Key {
                        keycode,
                        pressed: event.state == ElementState::Pressed,
                    });
                }
                _ => {}
            }
        }

        /// The network signaled that a frame arrived.
        ///
        /// Without this the image FROZE. The loop uses `ControlFlow::Wait`,
        /// which blocks waiting for a system event; the network thread
        /// filled the slot and woke nobody. `about_to_wait` runs BEFORE the
        /// loop sleeps, so a frame arriving after that waited for the next
        /// system event -- a mouse movement, a key. The user described
        /// exactly that: "it freezes on a chat message until I move the
        /// mouse".
        ///
        /// Request a redraw here rather than drawing directly: drawing
        /// outside `RedrawRequested` fights the compositor on some
        /// platforms, and the painting path already lives in one place.
        fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
            if let Some(j) = &self.window {
                j.request_redraw();
            }
        }

        fn about_to_wait(&mut self, _: &ActiveEventLoop) {
            // Safety net for a frame that arrived between the last draw
            // and the loop going to sleep. The `user_event` signal is the
            // normal path; this comparison covers the race.
            let g = self.slot.lock().map(|s| s.generation).unwrap_or(0);
            if g != self.drawn {
                if let Some(j) = &self.window {
                    j.request_redraw();
                }
            }
        }
    }

    impl App {
        /// `try_send` and not `send`: the window loop must not block
        /// waiting for the network -- it would freeze the whole UI.
        fn send(&self, e: Event) {
            let _ = self.tx_in.try_send(e);
        }

        fn draw(&mut self) {
            let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut())
            else {
                return;
            };
            // First make sure the scaling thread knows the current size:
            // on the first paint there has been no `Resized` yet.
            let physical = window.inner_size();
            if let Ok(mut g) = self.size.lock() {
                *g = (physical.width.max(1), physical.height.max(1));
            }
            let s = match self.slot.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if s.w == 0 || s.h == 0 {
                return;
            }
            // The surface is the size of the WINDOW, not the frame.
            //
            // Before, I sized it by the frame and ABANDONED the paint when
            // the two did not match. On entering full screen the window
            // grew, every frame in flight was discarded, and what stayed
            // on screen was the old buffer -- small, in the top-left
            // corner, not stretched. That is what the user saw.
            //
            // Now it ALWAYS paints: the window gets the background, the
            // image goes in its place, and a frame of a stale size shows
            // slightly off-scale for one frame instead of freezing the
            // screen. There is no stuck state anymore.
            let (Some(sw), Some(sh)) =
                (NonZeroU32::new(physical.width.max(1)), NonZeroU32::new(physical.height.max(1)))
            else {
                return;
            };
            if surface.resize(sw, sh).is_err() {
                return;
            }
            let Ok(mut buf) = surface.buffer_mut() else { return };
            let (win_w, win_h) = (physical.width as usize, physical.height as usize);
            buf.fill(BACKGROUND);
            let (ox, oy) = (s.ox as usize, s.oy as usize);
            let (iw, ih) = (s.w as usize, s.h as usize);
            for y in 0..ih {
                let dy = oy + y;
                if dy >= win_h {
                    break;
                }
                let width = iw.min(win_w.saturating_sub(ox));
                let src = y * iw;
                let dst = dy * win_w + ox;
                if src + width > s.rgb.len() || dst + width > buf.len() {
                    break;
                }
                buf[dst..dst + width]
                    .copy_from_slice(&s.rgb[src..src + width]);
            }
            self.drawn = s.generation;
            let captured = s.captured_ns;
            drop(s);
            let _ = buf.present();
            if self.measure {
                // Signed: between machines the host's clock may be ahead,
                // and clamping negatives to zero hid that.
                let now = logistream::serve::now_ns() as i128;
                self.delays.push((now - captured as i128) as f64 / 1e6);
                if self.delays.len() == 120 {
                    self.delays.sort_by(f64::total_cmp);
                    eprintln!(
                        "capture to screen: p50 {:.1} ms, p95 {:.1} ms",
                        self.delays[60], self.delays[114]
                    );
                    self.delays.clear();
                }
            }

            self.n += 1;
            if self.n % 120 == 0 {
                let secs = self.t0.elapsed().as_secs_f64();
                // In the log too, not just the title: this is exactly the
                // rate that sat at zero while the decoding rate climbed,
                // and a number that only exists in the window title is no
                // use for measuring anything remotely.
                println!("  {} frames drawn, {:.0} fps", self.n, self.n as f64 / secs);
                // The title is shown to the person: stays pt-BR.
                window.set_title(&format!(
                    "logistream — {:.0} fps desenhados",
                    self.n as f64 / secs
                ));
            }
        }
    }

    /// Receives, reassembles, decodes and leaves the result in the slot.
    #[tokio::main(flavor = "current_thread")]
    async fn receive(
        target: &str,
        slot: Arc<Mutex<Slot>>,
        mut rx_in: tokio::sync::mpsc::Receiver<Event>,
        size: Arc<Mutex<(u32, u32)>>,
        proxy: winit::event_loop::EventLoopProxy<()>,
    ) -> anyhow::Result<()> {
        // Decoder set up BEFORE connecting.
        //
        // It used to come after the connection, and that lost the first
        // keyframe: the host starts pouring out the IDR the instant the
        // handshake completes, and while we were setting up the decoder
        // the datagrams kept arriving. Whatever did not fit in the queue
        // was dropped, the IDR arrived incomplete, and the log filled with
        // `non-existing PPS 0 referenced`.
        let mut dec = logistream::decode::Decoder::new()
            .map_err(|e| anyhow::anyhow!("no decoder: {e}"))?;
        println!("decoding with {}", dec.name());

        let mut addrs: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(target).await?.collect();
        // IPv4 first: a `.local` name resolves in both families and the
        // local socket has to be born in the destination's family.
        addrs.sort_by_key(|a| !a.is_ipv4());
        let dest = *addrs.first().ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
        let bind: std::net::SocketAddr =
            if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
        let ep = net::client(bind)?;
        let conn = ep.connect(dest, "logistream")?.await?;
        println!("connected to {dest}");

        // Input over a unidirectional stream: reliable and ordered. Losing
        // a "key released" would leave the key stuck on the host.
        //
        // `open_uni` goes inside the task, not here: it costs a round
        // trip, and paying that before the datagram loop meant losing the
        // start of the video. Nobody types in the first millisecond;
        // whoever loses the keyframe sees a black screen.
        let conn_in = conn.clone();
        tokio::spawn(async move {
            let Ok(mut send) = conn_in.open_uni().await else { return };
            let mut buf = bytes::BytesMut::new();
            while let Some(e) = rx_in.recv().await {
                buf.clear();
                e.write(&mut buf);
                if send.write_all(&buf).await.is_err() {
                    break;
                }
            }
        });

        let mut r = Reassembler::default();
        let mut frames = 0u64;
        // Asks for a keyframe while there is nothing drawable
        // (net::KeyframeWatch). The read below times out so this gets a
        // chance to run on a still screen, where nothing arrives at all.
        let mut watch = net::KeyframeWatch::new(std::time::Instant::now());

        loop {
            if watch.should_request(std::time::Instant::now()) {
                let _ = conn.send_datagram(bytes::Bytes::from_static(net::KEYFRAME_REQUEST));
            }
            let d = match tokio::time::timeout(std::time::Duration::from_millis(100), conn.read_datagram()).await {
                Err(_) => continue,
                Ok(Ok(d)) => d,
                Ok(Err(e)) => {
                    println!("connection closed: {e}");
                    return Ok(());
                }
            };
            let pushed = r.push(&d);
            watch.losses(r.lost, std::time::Instant::now());
            let Some((hdr, unit)) = pushed else { continue };
            watch.frame(hdr.keyframe);
            // The first frames, one by one.
            //
            // "The image does not show up" has different causes with the
            // same symptoms, and only these three numbers tell them apart:
            // if the first whole frame is NOT a keyframe, the host sent a
            // P-frame to a client with no context; if it is a keyframe but
            // `lost` has already gone up, it arrived in pieces; if
            // everything is right here, the problem is the decoder's.
            if frames < 3 {
                println!(
                    "  received frame {} {} {} bytes, {} lost so far",
                    hdr.frame,
                    if hdr.keyframe { "KEY" } else { "P" },
                    unit.len(),
                    r.lost
                );
            }

            // The fit depends on the SOURCE size, which only the decoder
            // knows, and on the WINDOW size, which only the UI knows. Hence
            // passing the math as a function instead of a number: each
            // side contributes what it knows.
            let size_now = Arc::clone(&size);
            let mut fit_fn = move |rw: u32, rh: u32| {
                let (win_w, win_h) = size_now.lock().map(|g| *g).unwrap_or((rw, rh));
                fit(rw, rh, win_w.max(1), win_h.max(1))
            };
            let ready = match dec.feed(&unit, &mut fit_fn) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("decode: {e}");
                    continue;
                }
            };
            for pr in ready {
                if let Ok(mut s) = slot.lock() {
                    let (win_w, win_h) = size.lock().map(|g| *g).unwrap_or((pr.w, pr.h));
                    s.rgb = pr.rgb;
                    s.w = pr.w;
                    s.h = pr.h;
                    s.ox = pr.ox;
                    s.oy = pr.oy;
                    s.rw = pr.rw;
                    s.rh = pr.rh;
                    s.win_w = win_w;
                    s.win_h = win_h;
                    s.generation += 1;
                    s.captured_ns = hdr.captured_at_ns;
                }
                // Wake the UI. `Err` here means the window loop ended --
                // the person closed it -- and then there is no one left to
                // draw for.
                if proxy.send_event(()).is_err() {
                    return Ok(());
                }
                frames += 1;
                if frames % 120 == 0 {
                    println!("  {frames} frames decoded");
                }
            }
        }
    }
}
