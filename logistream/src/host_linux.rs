//! Host on Linux/Wayland: Mutter -> PipeWire -> NVENC -> QUIC.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why two threads and a channel
//!
//! The PipeWire loop is synchronous and owns its thread; `quinn` is async.
//! Calling the network from inside the frame callback would mix the two
//! worlds and, worse, any network hesitation would become a lost frame in
//! the capture. The channel decouples them: the callback encodes and
//! **lets go**; the network thread sends.
//!
//! The channel is shallow on purpose (`CAPACITY`). If the network cannot
//! keep up, the right thing is to **drop the old frame**, not to queue: on
//! a remote screen, a late frame has no value at all -- it is already past.

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
use pipewire::spa::utils::{Direction, Rectangle, SpaTypes};
use pipewire::stream::{Stream, StreamFlags};

use crate::input::{Event, Reader};
use crate::nvenc::NvencEncoder;
use crate::serve::{self, now_ns, EncodedFrame, Signals};

const SC: &str = "org.gnome.Mutter.ScreenCast";
const SC_OBJ: &str = "/org/gnome/Mutter/ScreenCast";
const SESSION: &str = "org.gnome.Mutter.ScreenCast.Session";
const STREAM: &str = "org.gnome.Mutter.ScreenCast.Stream";
const RD: &str = "org.gnome.Mutter.RemoteDesktop";
const RD_OBJ: &str = "/org/gnome/Mutter/RemoteDesktop";
const RD_SESSION: &str = "org.gnome.Mutter.RemoteDesktop.Session";
const T: Duration = Duration::from_secs(10);
/// Screen size when there is no monitor plugged into the machine.
const VIRTUAL_W: u32 = 1920;
const VIRTUAL_H: u32 = 1080;

/// Did Mutter reject the connector because it does not exist right now?
///
/// That is what happens with the monitor off or unplugged: the outputs go
/// `disconnected` and `RecordMonitor` answers "Unknown monitor". Any other
/// error (session, permission) is still an error.
fn monitor_missing(msg: Option<&str>) -> bool {
    msg.is_some_and(|m| m.contains("Unknown monitor"))
}

/// Asks Mutter for the stream: the requested monitor, or a virtual one if
/// it does not exist. Returns the stream path and whether it is virtual.
///
/// Without this, the machine with no monitor plugged in, which is exactly
/// the one you want to reach from afar, had no remote screen: the host died
/// at startup.
fn record(
    sp: &dbus::blocking::Proxy<'_, &Connection>,
    connector: &str,
) -> anyhow::Result<(Path<'static>, bool)> {
    let props = || {
        let mut p = dbus::arg::PropMap::new();
        p.insert(
            "cursor-mode".into(),
            dbus::arg::Variant(Box::new(1u32) as Box<dyn dbus::arg::RefArg>),
        );
        p
    };
    let request: Result<(Path,), dbus::Error> =
        sp.method_call(SESSION, "RecordMonitor", (connector, props()));
    match request {
        Ok((path,)) => Ok((path.into_static(), false)),
        Err(e) if monitor_missing(e.message()) => {
            eprintln!("no monitor {connector}: using virtual screen {VIRTUAL_W}x{VIRTUAL_H}");
            let (path,): (Path,) = sp.method_call(SESSION, "RecordVirtual", (props(),))?;
            Ok((path.into_static(), true))
        }
        Err(e) => Err(e.into()),
    }
}
/// Two frames of slack. More than that would be a queue of stale stuff.
const CAPACITY: usize = 2;

/// Target encoding cadence: one frame every 16.67 ms (60 Hz).
///
/// Mutter delivers at ~80 fps and we encoded them all. Above the refresh
/// rate of the viewer's screen that does not become picture -- it becomes
/// more bandwidth, more GPU here and more color conversion on the other
/// side, for nobody's eyes. The screens of all three machines are 60 Hz.
///
/// The gate compares against a DEADLINE that advances, not against the time
/// since the last frame. The first version did the latter and measured
/// 48 fps instead of 60: with the source delivering every 12.5 ms, "reject
/// if less than 16.7 ms" accepts one and rejects the next, and 80 becomes
/// ~40. With the advancing deadline, measured: 60.0 fps and 4.5 Mbps,
/// against 80 fps and 6.2 Mbps before.
const CADENCE: Duration = Duration::from_micros(16_667);

pub fn run(port: u16, connector: &str) -> anyhow::Result<()> {
    // A TOKIO channel, not `std::sync::mpsc`.
    //
    // With the std channel, the network loop blocked the thread in
    // `recv_timeout` -- and in a `current_thread` runtime that keeps the
    // connection driver from running. `send_datagram` only ENQUEUES; the
    // driver is what puts it on the wire. Measured result: host announcing
    // 80 fps and "0 refused" with the receiver at zero, because nothing was
    // being transmitted.
    let (tx, rx) = tokio::sync::mpsc::channel::<EncodedFrame>(CAPACITY);

    // --- choreography between the two threads ----------------------------
    //
    // The INPUT session has to exist before the SCREEN one: its `SessionId`
    // is what ties the two together, and without the link
    // `NotifyPointerMotionAbsolute` has no coordinate space.
    //
    // But the injector is the network thread, and in D-Bus a session
    // belongs to the connection that created it -- another connection
    // cannot call `Notify*` on it. So the input session is born THERE, and
    // only the identifier crosses over.
    //
    //   network: create input -> send SessionId ----> main
    //   main:    create linked screen, RecordMonitor -> send stream path
    //            (for the absolute pointer) -> network
    //   network: Start() -- which starts both
    //   main:    receive PipeWireStreamAdded and carry on
    let (tx_id, rx_id) = std::sync::mpsc::channel::<String>();
    let (tx_ready, rx_ready) = std::sync::mpsc::channel::<String>();

    // The one who decides to shut down is the network (nobody watching),
    // but the one who can STOP is the capture thread -- it owns the
    // PipeWire loop and the CUDA/NVENC resources.
    //
    // I tried `std::process::exit` from the network thread and the process
    // hung: exit starts destroying statics while capture is still encoding,
    // and the log showed `encode: InvalidParam` followed by nothing. A flag
    // plus a timer in the loop itself does it in the right order.
    let signals = std::sync::Arc::new(Signals::default());
    let signals_net = std::sync::Arc::clone(&signals);

    let network = std::thread::spawn(move || {
        if let Err(e) = run_network(port, rx, tx_id, rx_ready, std::sync::Arc::clone(&signals_net)) {
            eprintln!("  network: {e}");
        }
        // The network is done -- by success or by error. Either way there
        // is nobody left to send frames to, and the one who shuts down in
        // the right order is the capture. Without this line, a network
        // error left the process alive forever, capturing for nobody.
        signals_net.shutdown();
    });

    let rd_id = rx_id
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| anyhow::anyhow!("the network thread did not create the input session"))?;

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
    let (stream_path, virtual_screen) = record(&sp, connector)?;
    let node = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
    let node2 = std::sync::Arc::clone(&node);
    let mut rule = MatchRule::new_signal(STREAM, "PipeWireStreamAdded");
    rule.path = Some(stream_path.clone());
    conn.add_match(rule, move |(id,): (u32,), _, _| {
        *node2.lock().unwrap() = Some(id);
        true
    })?;
    // The one who calls Start is the network thread, owner of the input
    // session -- starting the input one starts the linked screen one.
    tx_ready.send(stream_path.to_string())?;
    let limit = Instant::now() + Duration::from_secs(5);
    while node.lock().unwrap().is_none() && Instant::now() < limit {
        conn.process(Duration::from_millis(100))?;
    }
    let node_id =
        (*node.lock().unwrap()).ok_or_else(|| anyhow::anyhow!("no PipeWireStreamAdded"))?;

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

    let enc = Rc::new(RefCell::new(None::<NvencEncoder>));
    let dropped = Rc::new(Cell::new(0u64));
    let next_frame_due = Rc::new(Cell::new(Instant::now()));
    let next_due_proc = Rc::clone(&next_frame_due);
    let enc_fmt = Rc::clone(&enc);
    let size_fmt = Cell::new((0u32, 0u32));
    let enc_proc = Rc::clone(&enc);
    let dropped_proc = Rc::clone(&dropped);
    // For the keyframe-request timer below: Mutter only delivers on change,
    // so on a still screen the timer is what answers the request.
    let enc_timer = Rc::clone(&enc);
    let tx_timer = tx.clone();
    let last_encoded = Rc::new(Cell::new(Instant::now()));
    let last_encoded_proc = Rc::clone(&last_encoded);
    let signals_proc = std::sync::Arc::clone(&signals);

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
            // The virtual screen renegotiates right after it is born, with
            // the same size. Swapping the encoder at that instant took the
            // process down with SIGSEGV (exit 139, measured); the physical
            // monitor negotiates only once and never went through here.
            if enc_fmt.borrow().is_some() && size_fmt.get() == (w, h) {
                return;
            }
            match NvencEncoder::new(w, h) {
                Ok(c) => {
                    println!("  capturing {w}x{h}, NVENC at {:.1} Mbps", c.target_mbps());
                    *enc_fmt.borrow_mut() = Some(c);
                    size_fmt.set((w, h));
                }
                Err(err) => eprintln!("  NVENC: {err}"),
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
            let captured = now_ns();

            // With nobody watching, do not encode: the frame would go into
            // a channel nobody drains. The PipeWire buffer was already
            // handed back above, so capture stays healthy.
            if !signals_proc.has_client() {
                return;
            }
            // Rate ceiling: dropping here is free (the buffer is already
            // back), and encoding costs GPU on both sides.
            let now = Instant::now();
            if now < next_due_proc.get() {
                return;
            }
            // `max(now, ...)`: after a pause (still screen, host with no
            // client) the deadline cannot be in the distant past, or the
            // first handful of frames would all go through together.
            next_due_proc.set((next_due_proc.get() + CADENCE).max(now));
            let mut g = enc_proc.borrow_mut();
            let Some(c) = g.as_mut() else { return };
            if signals_proc.take_idr() {
                c.request_idr();
            }
            match c.encode(px, stride) {
                // `try_send`: if the network cannot keep up, the new frame
                // is dropped HERE instead of becoming a queue. A late frame
                // has no value on a remote screen.
                Ok(data) => {
                    last_encoded_proc.set(Instant::now());
                    let keyframe = c.last_was_idr();
                    if tx.try_send(EncodedFrame { data, keyframe, captured_at_ns: captured }).is_err() {
                        dropped_proc.set(dropped_proc.get() + 1);
                    }
                }
                Err(err) => eprintln!("  {err}"),
            }
        })
        .register()?;

    let mut obj = pod::object!(
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
    // A real monitor dictates the size. The virtual one has no size at all
    // until the client asks for one, and without asking it is born 1280x720.
    if virtual_screen {
        obj.properties.push(pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle { width: VIRTUAL_W, height: VIRTUAL_H },
            Rectangle { width: 640, height: 480 },
            Rectangle { width: 7680, height: 4320 }
        ));
    }
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

    // A timer in the loop itself: it is the only place `quit` can be called
    // from safely, because the `MainLoop` does not cross threads.
    let ml = mainloop.clone();
    let signals_timer = std::sync::Arc::clone(&signals);
    let timer = mainloop.loop_().add_timer(move |_| {
        if signals_timer.should_exit() {
            ml.quit();
            return;
        }
        // A keyframe was requested and no frame came to carry it: the
        // screen is still. Re-encode the last frame as an IDR. `take_idr`
        // goes last so the request is only consumed when we answer it.
        if signals_timer.has_client()
            && signals_timer.idr_pending()
            && last_encoded.get().elapsed() >= Duration::from_millis(80)
            && signals_timer.take_idr()
        {
            let mut g = enc_timer.borrow_mut();
            let Some(c) = g.as_mut() else { return };
            match c.reencode_idr() {
                Ok(data) => {
                    last_encoded.set(Instant::now());
                    let frame = EncodedFrame { data, keyframe: true, captured_at_ns: now_ns() };
                    if tx_timer.try_send(frame).is_err() {
                        eprintln!("  keyframe for a still screen dropped: the network queue is full");
                    }
                }
                Err(e) => eprintln!("  keyframe for a still screen: {e}"),
            }
        }
    });
    // 100 ms: it also answers keyframe requests on a still screen, and the
    // viewer asks every 300 ms.
    timer
        .update_timer(Some(Duration::from_millis(100)), Some(Duration::from_millis(100)))
        .into_result()?;

    println!("  host on port {port}; ctrl-c to stop");
    mainloop.run();
    println!("  capture ended");
    let _ = sp.method_call::<(), _, _, _>(SESSION, "Stop", ());
    let _ = network.join();
    Ok(())
}

/// Injects an event into the Mutter session, WITHOUT waiting for a reply.
///
/// `NotifyPointerMotionAbsolute` takes the path of the screen STREAM: it is
/// what defines the coordinate space. That is why the input session has to
/// be linked to the screen one -- without the link, there is nowhere to map
/// the x,y to.
///
/// ## Why `no_reply`, and not `method_call`
///
/// It was `method_call`, which WAITS for the reply. The user described both
/// symptoms of that in the same sentence: "mac para ubuntu meeeeeeeio
/// bugado e lento o iiiiiiiinput" ("mac to ubuntu kiiiiind of buggy and
/// slow, the iiiiiinput") -- the repeated letters are a stuck key, the "key
/// up" arriving late -- and "o video para se eu paro de digitar" ("the
/// video stops if I stop typing").
///
/// One cause, two symptoms. The thread that injects input is the SAME one
/// that pumps the video datagrams (see `run_network`: a single-thread
/// runtime, because the D-Bus connection is not Send). Each event stopped
/// there waiting for Mutter to reply -- Mutter, which at that instant is
/// also capturing and encoding. While the reply did not come, no frame went
/// out; and while frames went out, the "key up" waited.
///
/// The reply to an input injection carries nothing we use. With
/// `NO_REPLY_EXPECTED` the call becomes a socket write and there is no
/// more round trip on the hot path.
///
/// The price is real and accepted: injection errors stop being visible per
/// event. What remains is the WRITE error, which is what signals a dead
/// session -- the case that matters.
fn apply_event(conn: &Connection, rd: &Path, stream: &str, e: Event) -> Result<(), String> {
    use crate::input::Event as E;
    let m = |member: &str| {
        dbus::Message::new_method_call(RD, rd.clone(), RD_SESSION, member)
    };
    let msg = match e {
        E::Mouse { x, y } => m("NotifyPointerMotionAbsolute").map(|z| z.append3(stream, x, y)),
        // Relative has its own method in Mutter, and is not the absolute one
        // with the math done: `NotifyPointerMotion` honors acceleration and
        // the screen bounds on this side, which is what the KVM wants.
        E::MouseRel { dx, dy } => m("NotifyPointerMotion").map(|z| z.append2(dx, dy)),
        E::Button { code, pressed } => {
            m("NotifyPointerButton").map(|z| z.append2(code, pressed))
        }
        // flags 0 = continuous scroll. `AxisDiscrete` would be for a wheel
        // with clicks, but the Mac trackpad has no steps.
        E::Scroll { dx, dy } => m("NotifyPointerAxis").map(|z| z.append3(dx, dy, 0u32)),
        E::Key { keycode, pressed } => {
            m("NotifyKeyboardKeycode").map(|z| z.append2(keycode, pressed))
        }
    };
    let mut msg = msg.map_err(|e| format!("build message: {e}"))?;
    msg.set_no_reply(true);
    conn.channel().send(msg).map_err(|_| "D-Bus queue full".to_string())?;
    Ok(())
}

/// Network: serves video by datagram and receives input by stream.
///
/// `LocalSet` and not `tokio::spawn` because the D-Bus connection **is not
/// Send**, and the input session belongs to the connection that created it
/// -- a second connection calling `Notify*` on that session would be
/// refused by Mutter. So everything runs on this thread and the `Rc`
/// shares it.
fn run_network(
    port: u16,
    mut rx: tokio::sync::mpsc::Receiver<EncodedFrame>,
    tx_id: std::sync::mpsc::Sender<String>,
    rx_ready: std::sync::mpsc::Receiver<String>,
    signals: std::sync::Arc<Signals>,
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

        // Wait for the screen session to exist, already linked to this one.
        let stream_path = rx_ready.recv_timeout(Duration::from_secs(10))?;
        dbus.with_proxy(RD, rd_path.clone(), T)
            .method_call::<(), _, _, _>(RD_SESSION, "Start", ())?;
        println!("  input enabled (stream {stream_path})");

        let listen = crate::bind::listen_addr();
        let ep = crate::net::server((listen, port).into())?;
        // The REAL address, not a hardcoded "0.0.0.0": with --bind overlay
        // the host listens somewhere else, and a log saying 0.0.0.0 would
        // send whoever is diagnosing to look in the wrong place.
        println!("  listening on {listen}:{port}");

        // Input port, next to the video one.
        //
        // The host serves one VIDEO client at a time, and the LAN KVM
        // fought the viewer for that slot: the user sent input to the
        // other machine and saw its screen frozen, concluding (rightly,
        // from what they saw) that nothing had moved.
        let input_port = port + 1;
        let ep_in = crate::net::server((listen, input_port).into())?;
        println!("  input on {listen}:{input_port}");
        {
            let signals_in = std::sync::Arc::clone(&signals);
            let dbus_in = Rc::clone(&dbus);
            let rd_i = rd_path.clone();
            let st_i = stream_path.clone();
            tokio::task::spawn_local(async move {
                serve::serve_input(&ep_in, std::sync::Arc::clone(&signals_in), |conn| {
                    let conn = conn.clone();
                    let dbus = Rc::clone(&dbus_in);
                    let rd = rd_i.clone();
                    let st = st_i.clone();
                    tokio::task::spawn_local(
                        async move { receive_input(conn, dbus, rd, st).await },
                    )
                })
                .await;
            });
        }

        serve::serve_video(&ep, &mut rx, &signals, |conn| {
            let conn = conn.clone();
            let dbus = Rc::clone(&dbus);
            let rd_in = rd_path.clone();
            let stream_in = stream_path.clone();
            tokio::task::spawn_local(
                async move { receive_input(conn, dbus, rd_in, stream_in).await },
            )
        })
        .await
    })
}

/// Reads a connection's input stream and injects until it closes.
///
/// One for both ports: the viewer sends input over the video connection,
/// the KVM sends it over the input port, and both deserve the same
/// treatment. Duplicating this would invite the two copies to diverge --
/// and one of them is the one the user types with.
async fn receive_input(
    conn: quinn::Connection,
    dbus: Rc<Connection>,
    rd_in: Path<'static>,
    stream_in: String,
) {
    let Ok(mut recv) = conn.accept_uni().await else { return };
    let mut reader = Reader::default();
    let mut buf = vec![0u8; 4096];
    let mut n = 0u64;
    let mut failures = 0u64;
    while let Ok(Some(read)) = recv.read(&mut buf).await {
        reader.feed(&buf[..read]);
        let mut applied_now = 0u32;
        while let Some(e) = reader.next_event() {
            if let Err(err) = apply_event(&dbus, &rd_in, &stream_in, e) {
                failures += 1;
                // Once, then every hundred: injection at 60 Hz would turn
                // the log into noise, and the number that matters is
                // "stopped working", not each event.
                if failures == 1 || failures % 100 == 0 {
                    eprintln!("  input refused ({failures}): {err}");
                }
            }
            n += 1;
            // The first ones, one by one: "no error" is weak proof that
            // the input arrived. After that only the aggregate, so as not
            // to flood the log at 60 Hz.
            if n <= 8 {
                println!("  input #{n}: {e:?}");
            } else if n % 500 == 0 {
                println!("  {n} input events applied");
            }
            applied_now += 1;
        }

        // FLUSH THE QUEUE. Without this the input does not leave here.
        //
        // `Channel::send` calls `dbus_connection_send` and nothing
        // else; what writes to the socket is
        // `dbus_connection_flush`, which lives in `flush()` --
        // separately. (The crate's other API, `ffidisp`, does both
        // together; ours does not.) Without a flush, each event sat
        // in libdbus's outgoing queue until something else
        // triggered a write.
        //
        // The symptom was not "input does not work", it was worse:
        // it worked in bursts. A key-up delayed past ~500 ms makes
        // Mutter go into auto-repeat, and the user wrote
        // "winddowsssss" and "nnnnnnnnnn" -- the repeated letters in
        // their message were the bug.
        //
        // Once per network read, not per event: one `read` brings
        // the whole batch of a mouse movement, and flushing per
        // event would be a wasted syscall on the hot path. It does
        // not wait for a reply from Mutter -- it only writes the
        // socket -- so it keeps the reason for
        // `set_no_reply(true)`.
        if applied_now > 0 {
            dbus.channel().flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::monitor_missing;

    #[test]
    fn only_missing_monitor_falls_back_to_virtual() {
        assert!(monitor_missing(Some("Unknown monitor")));
        assert!(!monitor_missing(Some("Permission denied")));
        assert!(!monitor_missing(None));
    }
}
