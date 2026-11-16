// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! LAN KVM: captures keyboard and mouse here and injects them on the other machine.
//!
//! Spec: ../../specs/005-input-switch.md (Linux), ../../specs/024-windows-kvm-source.md (Windows)
//!
//!   oxdesk-kvm <host:port> [--observe] [--no-repeat] [--rescue A,B] [--device NAME]...
//!
//! It does not matter which machine has the dongle: what matters is where
//! the devices are connected NOW. Switching machines stops being a radio
//! command to the keyboard (HID++ 0x1814) and becomes a software decision
//! -- instant, and reversible without touching anything.
//!
//! Linux grabs with `EVIOCGRAB`; Windows with low-level hooks
//! (`grab_win.rs`). Everything after the grab -- the forwarding, the stuck
//! keys, the reasons for stopping -- is the same code for both.
//!
//! ## The order of the two dangerous operations
//!
//! **Connect first, grab after.** Grabbed, the keyboard does not exist for
//! the local desktop; if the target were down and we had already grabbed,
//! the person would be left without a keyboard AND without a destination.
//! So: the connection failed, nobody grabs and we exit with an error.
//!
//! **Release before dying, always.** Every exit path releases everything.
//! The system also releases when the process ends (the fd closes, the hooks
//! go with the process), so a keyboard locked forever is not reachable --
//! but the keys that were PRESSED on the other side would stay stuck, and
//! that is reachable. That is why we send a key-up for whatever is pressed
//! before exiting.
//!
//! ## Rescue
//!
//! Two keys together -- both Ctrl unless `--rescue` names another pair --
//! release everything and exit. There has to be a way out that does not
//! depend on the network: if the target goes down with the grab active,
//! neither keyboard nor mouse reach the local desktop.
//!
//! The daemon shows the last line of this process's log when it exits
//! within its first 500 ms ("the KVM exited (...) before grabbing: {line}",
//! oxdesk/src/kvm.rs), so every fatal line says what happened in full.

#[cfg(any(target_os = "linux", windows))]
use std::collections::HashSet;
#[cfg(any(target_os = "linux", windows))]
use std::time::Duration;

#[cfg(any(target_os = "linux", windows))]
use bytes::BytesMut;
#[cfg(any(target_os = "linux", windows))]
use oxdesk_stream::input::{
    chord_held, parse_chord, parse_size_line, Edge, Event, Pointer, Step, RESCUE_DEFAULT,
};
#[cfg(any(target_os = "linux", windows))]
use oxdesk_stream::net;

#[cfg(target_os = "linux")]
use oxdesk_stream::capture_input::Device;

/// Default names, if nobody passes `--device`.
#[cfg(target_os = "linux")]
const DEFAULT: [&str; 2] = ["MX Keys", "MX Anywhere"];

#[cfg(not(any(target_os = "linux", windows)))]
fn main() {
    eprintln!("the KVM can capture on Linux and Windows only");
    std::process::exit(1);
}

/// The command line, the same on both systems.
#[cfg(any(target_os = "linux", windows))]
struct Args {
    target: String,
    observe: bool,
    repeat: bool,
    self_test: usize,
    rescue: (u32, u32),
    /// The other screen's edge that faces this machine (`--return-edge`):
    /// pushing the mouse past it brings the input home. The daemon passes
    /// it when the edge is on and the layout says where the target sits.
    return_edge: Option<Edge>,
    #[cfg_attr(windows, allow(dead_code))]
    filters: Vec<String>,
}

#[cfg(any(target_os = "linux", windows))]
fn parse_args() -> anyhow::Result<Args> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The old spellings answer too. `oxdesk` launches this binary, so the
    // two travel together -- but a developer with a shell history and the
    // notes in specs/ are also callers, and a flag that silently stops
    // being recognised here means grabbing the keyboard with the wrong
    // repeat setting.
    let has = |new: &str, old: &str| args.iter().any(|a| a == new || a == old);
    let value = |name: &str, old: &str| {
        args.iter().position(|a| a == name || a == old).and_then(|i| args.get(i + 1)).cloned()
    };
    // A target that applies its own repeat logic (Mutter does that with
    // injected keys) would get doubled repeat if we sent ours along.
    // Whoever launches the process knows the target's OS.
    let repeat = !has("--no-repeat", "--sem-repeticao");
    // `--self-test N`: connects, does NOT grab, and pushes N movements
    // through the same bridge a device would use (thread -> channel ->
    // async loop -> QUIC). It exists because the bug it caught lived
    // exactly in that bridge: `recv_timeout` blocking the executor kept the
    // QUIC driver from transmitting. Testing with the real keyboard would
    // require grabbing it, and grabbing to test is precisely what you do not
    // want to do on someone's machine.
    let self_test: usize =
        value("--self-test", "--autoteste").and_then(|v| v.parse().ok()).unwrap_or(0);
    // A rescue we cannot read is refused, not replaced by the default: the
    // person was told which keys bring the input back, and grabbing with
    // other keys would make that sentence false.
    let rescue = match value("--rescue", "--rescue") {
        Some(v) => parse_chord(&v).ok_or_else(|| {
            anyhow::anyhow!("--rescue '{v}' is not two different key codes (e.g. 29,97). Nothing was grabbed.")
        })?,
        None => RESCUE_DEFAULT,
    };
    let mut skip_next = false;
    let mut target = None;
    let mut filters: Vec<String> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        match a.as_str() {
            "--device" | "--dispositivo" => {
                if let Some(n) = args.get(i + 1) {
                    filters.push(n.clone());
                }
                skip_next = true;
            }
            "--self-test" | "--autoteste" | "--rescue" | "--return-edge" => skip_next = true,
            s if s.starts_with("--") => {}
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                }
            }
        }
    }
    Ok(Args {
        target: target.unwrap_or_else(|| "127.0.0.1:9777".into()),
        observe: has("--observe", "--observar"),
        repeat,
        self_test,
        rescue,
        return_edge: value("--return-edge", "--return-edge").as_deref().and_then(Edge::parse),
        filters,
    })
}

/// Connects to the target's input port. Failing here is the GOOD case:
/// nobody has grabbed anything yet.
#[cfg(any(target_os = "linux", windows))]
async fn connect(target: &str) -> anyhow::Result<(quinn::Connection, quinn::SendStream)> {
    let mut addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(target).await?.collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    let dest = *addrs.first().ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
    let bind: std::net::SocketAddr =
        if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::client(bind)?;
    let c = ep
        .connect(dest, "oxdesk-stream")?
        .await
        .map_err(|e| anyhow::anyhow!("could not connect to {target}: {e}. Nothing was grabbed."))?;
    println!("  connected to {target}");
    let s = c.open_uni().await?;
    Ok((c, s))
}

/// Listens for the size the host announces on a stream of its own
/// (`input::size_line`). A host from before it never sends one, and the
/// slot stays empty: motion then stays relative, as it always was.
#[cfg(any(target_os = "linux", windows))]
fn spawn_size_reader(conn: &quinn::Connection) -> std::sync::Arc<std::sync::Mutex<Option<(u32, u32)>>> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (c, s) = (conn.clone(), std::sync::Arc::clone(&slot));
    tokio::spawn(async move {
        let Ok(Ok(mut r)) = tokio::time::timeout(Duration::from_secs(5), c.accept_uni()).await else {
            return;
        };
        let Ok(bytes) = r.read_to_end(64).await else { return };
        match parse_size_line(&String::from_utf8_lossy(&bytes)) {
            Some((w, h)) => {
                println!("  the other screen is {w}x{h}");
                if let Ok(mut g) = s.lock() {
                    *g = Some((w, h));
                }
            }
            None => eprintln!("  the other side sent a size line this build does not read"),
        }
    });
    slot
}

/// The movements `--self-test` pushes, from a thread, like a device would.
#[cfg(any(target_os = "linux", windows))]
fn spawn_self_test(n: usize, tx: impl Fn(Event) -> bool + Send + 'static) {
    std::thread::spawn(move || {
        for _ in 0..n {
            if !tx(Event::MouseRel { dx: 10.0, dy: 0.0 }) {
                return;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    });
    println!("  self-test: {n} movements of +10 in X, without grabbing");
}

/// Why forwarding stopped, and the keys still held on the other side.
#[cfg(any(target_os = "linux", windows))]
struct Stopped {
    reason: String,
    pressed: HashSet<u32>,
    /// What was forwarded, by kind: keys, buttons, moves, scrolls. Printed
    /// on exit, because "nothing arrived on the other side" has two causes
    /// -- nobody touched anything, or the capture saw nothing -- and only
    /// this number tells them apart.
    sent: [usize; 4],
    /// Came home by the edge: where along it, 0..1.
    returned: Option<f64>,
}

/// Forwards events until the source ends, the connection drops or the
/// rescue chord is held.
///
/// `closed` names the reason when the channel closes: on Linux that is the
/// device threads dying; on Windows it is the grab ending, which is how the
/// hook thread reports the rescue it already acted on.
#[cfg(any(target_os = "linux", windows))]
async fn forward(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    connection: Option<&quinn::Connection>,
    send: &mut Option<quinn::SendStream>,
    observe: bool,
    rescue: (u32, u32),
    pointer: &mut Pointer,
    size: &std::sync::Mutex<Option<(u32, u32)>>,
    closed: impl Fn() -> String,
) -> Stopped {
    let mut buf = BytesMut::new();
    let mut pressed: HashSet<u32> = HashSet::new();
    let mut sent = [0usize; 4];
    let mut returned = None;
    let reason = loop {
        let e = match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(e)) => e,
            Ok(None) => break closed(),
            Err(_) => {
                if let Some(r) = connection.and_then(|c| c.close_reason()) {
                    break format!("the connection dropped: {r}");
                }
                continue;
            }
        };

        // Stuck-key bookkeeping. It has to apply ALSO in `--observe`,
        // otherwise the first real use starts with the set lying.
        if let Event::Key { keycode, pressed: down } = e {
            if down {
                pressed.insert(keycode);
            } else {
                pressed.remove(&keycode);
            }
        }

        // On Windows the hook has already decided this before queuing
        // (grab_win.rs), so it never fires there; on Linux it is the rescue.
        if chord_held(&pressed, rescue) {
            break "rescue: the rescue keys were held together".into();
        }

        if observe {
            println!("  {e:?}");
            continue;
        }

        // The other screen's size, once it has arrived: from then on the
        // cursor's position is kept here and sent absolute, starting with
        // the point it enters at (see `input::Pointer`).
        if pointer.wants_size() {
            if let Some((w, h)) = size.lock().ok().and_then(|g| *g) {
                if let Some(first) = pointer.set_size(w, h) {
                    buf.clear();
                    first.write(&mut buf);
                    if let Some(s) = send.as_mut() {
                        if let Err(err) = s.write_all(&buf).await {
                            break format!("write failed: {err}");
                        }
                    }
                }
            }
        }
        let e = match pointer.map(e) {
            Step::Send(e) => e,
            Step::Return(along) => {
                returned = Some(along);
                break "home by the edge: the mouse was pushed past the edge that faces this machine".into();
            }
        };

        buf.clear();
        e.write(&mut buf);
        if let Some(s) = send.as_mut() {
            if let Err(err) = s.write_all(&buf).await {
                break format!("write failed: {err}");
            }
        }
        sent[match e {
            Event::Key { .. } => 0,
            Event::Button { .. } => 1,
            Event::Mouse { .. } | Event::MouseRel { .. } => 2,
            Event::Scroll { .. } => 3,
        }] += 1;
    };
    Stopped { reason, pressed, sent, returned }
}

/// Releases on the other side whatever is still held there.
///
/// Without this, exiting with Shift pressed leaves the other machine
/// typing in uppercase forever. It already happened on the viewer path
/// (Mac -> Ubuntu) and cost a session restart.
#[cfg(any(target_os = "linux", windows))]
async fn finish(send: &mut Option<quinn::SendStream>, stopped: &Stopped) {
    if let Some(s) = send.as_mut() {
        let mut buf = BytesMut::new();
        for k in &stopped.pressed {
            buf.clear();
            Event::Key { keycode: *k, pressed: false }.write(&mut buf);
            let _ = s.write_all(&buf).await;
        }
        let _ = s.finish();
    }
    // One turn so the last bytes go out before the socket closes.
    tokio::time::sleep(Duration::from_millis(120)).await;
    let [k, b, m, w] = stopped.sent;
    println!("  forwarded: {k} key, {b} button, {m} move, {w} scroll event(s)");
    println!(
        "  {}; {} key(s) released; control is back here",
        stopped.reason,
        stopped.pressed.len()
    );
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let a = parse_args()?;
    let mut filters = a.filters.clone();
    if filters.is_empty() {
        filters = DEFAULT.iter().map(|s| s.to_string()).collect();
    }

    // --- choose devices --------------------------------------------
    let all = Device::list();
    let mut chosen: Vec<Device> = Vec::new();
    for mut d in all {
        if filters.iter().any(|f| d.name.contains(f.as_str())) {
            println!("  {} -> {}", d.path.display(), d.name);
            d.set_repeat(a.repeat);
            chosen.push(d);
        }
    }
    // Say which filter found nothing.
    //
    // A Logitech device disappears from the receiver when it sleeps or is
    // turned off, and the /dev/input node disappears with it. Without this
    // warning, the KVM would take only the keyboard and the mouse would stay
    // here -- and the symptom would be "half of it works", which is harder
    // to understand than "nothing works". It happened in a test: the mouse
    // was asleep and only the keyboard went over, silently.
    for f in &filters {
        if !chosen.iter().any(|d| d.name.contains(f.as_str())) {
            eprintln!("  WARNING: nothing matched '{f}' -- device asleep or turned off?");
        }
    }

    if chosen.is_empty() {
        anyhow::bail!(
            "no device matched {filters:?}. No permission? \
             See .claude/skills/grant-input-access.md -- without the udev \
             rule the nodes exist but do not open, and the list comes out \
             empty exactly as if the device were not connected."
        );
    }

    // --- connect BEFORE grabbing -----------------------------------
    let (connection, mut send) = if a.observe {
        println!("  observe mode: does not grab and sends nothing");
        (None, None)
    } else {
        let (c, s) = connect(&a.target).await?;
        (Some(c), Some(s))
    };

    // --- grab -------------------------------------------------------
    if !a.observe && a.self_test == 0 {
        for d in &mut chosen {
            d.grab().map_err(|e| anyhow::anyhow!("could not grab {}: {e}", d.name))?;
        }
        println!("  grabbed; the rescue keys ({},{}) give control back", a.rescue.0, a.rescue.1);
    }

    // --- read in threads, one per device ----------------------------
    //
    // One thread per device because `read` on evdev blocks, and the
    // keyboard cannot wait for the mouse to move.
    //
    // TOKIO channel, not std. The first version used `std::sync::mpsc`
    // and `recv_timeout` in the main loop. `recv_timeout` BLOCKS the thread
    // -- and in a `current_thread` runtime the thread is the whole
    // executor, including the QUIC driver, which is what actually puts the
    // packets on the network. It seemed to work: now and then a handful of
    // events escaped in the window where the driver got to run, and the
    // host logged "input connected" and soon "input disconnected" without a
    // single event applied in between.
    //
    // Unbounded so the Windows hook (which must never block) and these
    // threads feed the same loop; a keyboard and a mouse cannot outrun it.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    if a.self_test > 0 {
        let tx = tx.clone();
        spawn_self_test(a.self_test, move |e| tx.send(e).is_ok());
    }

    for mut d in chosen {
        if a.self_test > 0 {
            break; // do not read a real device in the self-test
        }
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match d.read() {
                Ok(events) => {
                    for e in events {
                        if tx.send(e).is_err() {
                            return; // main exited; releasing comes from Drop
                        }
                    }
                }
                Err(err) => {
                    eprintln!("  {} stopped: {err}", d.name);
                    return;
                }
            }
        });
    }
    drop(tx);

    let size = match &connection {
        Some(c) => spawn_size_reader(c),
        None => std::sync::Arc::new(std::sync::Mutex::new(None)),
    };
    // evdev does not say where the cursor is, so it enters halfway along.
    let mut pointer = Pointer::new(a.return_edge, 0.5);
    let stopped = forward(
        &mut rx,
        connection.as_ref(),
        &mut send,
        a.observe,
        a.rescue,
        &mut pointer,
        &size,
        || "the devices stopped answering".into(),
    )
    .await;
    finish(&mut send, &stopped).await;
    Ok(())
}

#[cfg(windows)]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    use oxdesk_stream::grab_win::{self, Grab, GrabOptions};

    let a = parse_args()?;
    // Per-monitor DPI awareness before anything reads a coordinate: the
    // hook reports physical pixels, and `SetCursorPos` in a DPI-unaware
    // process takes logical ones. At 100% scale the two agree, which is
    // exactly how a mismatch would hide until someone changed the scale
    // (.claude/rules/coordinate-spaces.md).
    // SAFETY: a process-wide setting, called once at startup.
    unsafe {
        use windows::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    if a.observe {
        anyhow::bail!("--observe is Linux only: on Windows watching means hooking, which is grabbing");
    }

    // --- connect BEFORE grabbing -----------------------------------
    let (connection, send) = connect(&a.target).await?;
    let mut send = Some(send);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // --- grab -------------------------------------------------------
    let grab = if a.self_test > 0 {
        spawn_self_test(a.self_test, move |e| tx.send(e).is_ok());
        None
    } else {
        let g = Grab::start(GrabOptions { rescue: a.rescue, repeat: a.repeat }, tx)
            .map_err(|e| anyhow::anyhow!("could not grab: {e}. Nothing was sent."))?;
        println!("  grabbed; the rescue keys ({},{}) give control back", a.rescue.0, a.rescue.1);
        Some(g)
    };

    // The daemon ends a grab by closing our stdin: a pipe it holds open
    // for as long as it wants the input away. EOF -- because it asked, or
    // because it died -- ends the grab the same way the rescue does, so the
    // keys held on the other side are released. Killing the process would
    // skip that.
    if let Some(g) = &grab {
        let stop = StopHandle(g.thread());
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            use std::io::Read;
            let mut stdin = std::io::stdin();
            while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
            stop.stop();
        });
    }

    let size = spawn_size_reader(&connection);
    // The cursor enters the other screen at the height (or the place along
    // a top/bottom edge) it left this one at.
    let entry = match (&grab, a.return_edge) {
        (Some(g), Some(Edge::Top | Edge::Bottom)) => g.entry_ratio().0,
        (Some(g), Some(_)) => g.entry_ratio().1,
        _ => 0.5,
    };
    let mut pointer = Pointer::new(a.return_edge, entry);
    let stopped = forward(
        &mut rx,
        Some(&connection),
        &mut send,
        false,
        a.rescue,
        &mut pointer,
        &size,
        || {
            if grab_win::rescued() {
                "rescue: the rescue keys were held together".into()
            } else if a.self_test > 0 {
                "self-test done".into()
            } else {
                "the grab ended".into()
            }
        },
    )
    .await;
    // Home by the edge: the cursor comes in by the matching edge here, at
    // the same place along it -- not where it was when it left.
    let mut grab = grab;
    if let (Some(along), Some(edge), Some(g)) = (stopped.returned, a.return_edge, grab.as_mut()) {
        // Their right edge faces us: we are to their right, so we come in
        // by our left edge. And so on round. On a real monitor at that
        // height, not on the box around them (`input::return_point`).
        let here = match edge {
            Edge::Right => Edge::Left,
            Edge::Left => Edge::Right,
            Edge::Bottom => Edge::Top,
            Edge::Top => Edge::Bottom,
        };
        if let Some((x, y)) = oxdesk_stream::input::return_point(here, along, &grab_win::monitors(), 60) {
            g.restore_at(x, y);
        }
    }
    // Unhook before the slow part: the person gets the keyboard back now,
    // and the key-ups for the other side go out after.
    let hooked = grab.is_some();
    drop(grab);
    if hooked {
        println!("  hooks passed {} injected event(s) through", grab_win::passed_through());
        println!("  {} release(s) of keys or buttons held from before the grab went to this machine", grab_win::kept_home());
    }
    finish(&mut send, &stopped).await;
    Ok(())
}

/// Ends a grab from the stdin thread, which cannot hold the `Grab` itself.
#[cfg(windows)]
struct StopHandle(u32);

#[cfg(windows)]
impl StopHandle {
    fn stop(&self) {
        oxdesk_stream::grab_win::stop_thread(self.0);
    }
}
