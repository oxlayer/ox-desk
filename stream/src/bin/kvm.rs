//! LAN KVM: captures keyboard and mouse here and injects them on the other machine.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//!   oxdesk-kvm <host:port> [--observar] [--dispositivo NAME]...
//!
//! It does not matter which machine has the dongle: what matters is where
//! the devices are connected NOW. Switching machines stops being a radio
//! command to the keyboard (HID++ 0x1814) and becomes a software decision
//! -- instant, and reversible without touching anything.
//!
//! ## The order of the two dangerous operations
//!
//! **Connect first, grab after.** Grabbed, the keyboard does not exist for
//! the local desktop; if the target were down and we had already grabbed,
//! the person would be left without a keyboard AND without a destination.
//! So: the connection failed, nobody grabs and we exit with an error.
//!
//! **Release before dying, always.** Every exit path releases everything.
//! The kernel also releases when the fd closes, so a keyboard locked
//! forever is not reachable -- but the keys that were PRESSED on the other
//! side would stay stuck, and that is reachable. That is why we send a
//! key-up for whatever is pressed before exiting.
//!
//! ## Rescue
//!
//! Both Ctrl keys together release everything and exit. There has to be a
//! way out that does not depend on the network: if the target goes down
//! with the grab active, neither keyboard nor mouse reach the local desktop.
//!
//! The messages printed right before a fatal exit stay in pt-BR: `oxdesk`
//! shows the last line of this process's log to the person when it exits
//! early ("o KVM saiu (...) antes de agarrar: {line}", oxdesk/src/kvm.rs).

// All of these are consumed by the Linux `main` alone. Without the cfg
// they come out as four `unused import` warnings on the Windows build --
// which nobody saw while that build was not part of check.sh.
#[cfg(target_os = "linux")]
use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use bytes::BytesMut;
#[cfg(target_os = "linux")]
use oxdesk_stream::input::Event;
#[cfg(target_os = "linux")]
use oxdesk_stream::net;

#[cfg(target_os = "linux")]
use oxdesk_stream::capture_input::{is_rescue, Device};

/// Default names, if nobody passes `--dispositivo`.
#[cfg(target_os = "linux")]
const DEFAULT: [&str; 2] = ["MX Keys", "MX Anywhere"];

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("o KVM so captura no Linux por enquanto");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let observe = args.iter().any(|a| a == "--observar");
    // A target that applies its own repeat logic (Mutter does that with
    // injected keys) would get doubled repeat if we sent the kernel's
    // along. Whoever launches the process knows the target's OS.
    let repeat = !args.iter().any(|a| a == "--sem-repeticao");
    // `--autoteste N`: connects, does NOT grab, and pushes N movements
    // through the same bridge a device would use (thread -> channel ->
    // async loop -> QUIC). It exists because the bug it caught lived
    // exactly in that bridge: `recv_timeout` blocking the executor kept the
    // QUIC driver from transmitting. Testing with the real keyboard would
    // require grabbing it, and grabbing to test is precisely what you do not
    // want to do on someone's machine.
    let self_test: usize = args
        .iter()
        .position(|a| a == "--autoteste")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let target = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:9777".into());

    let mut filters: Vec<String> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--dispositivo" {
            if let Some(n) = args.get(i + 1) {
                filters.push(n.clone());
            }
        }
    }
    if filters.is_empty() {
        filters = DEFAULT.iter().map(|s| s.to_string()).collect();
    }

    // --- choose devices --------------------------------------------
    let all = Device::list();
    let mut chosen: Vec<Device> = Vec::new();
    for mut d in all {
        if filters.iter().any(|f| d.name.contains(f.as_str())) {
            println!("  {} -> {}", d.path.display(), d.name);
            d.set_repeat(repeat);
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

    // pt-BR on purpose: fatal before grabbing, shown in the tray (see the
    // module doc).
    if chosen.is_empty() {
        anyhow::bail!(
            "nenhum dispositivo casou com {filters:?}. Sem permissao? \
             Veja .claude/skills/grant-input-access.md -- sem a regra de \
             udev os nos existem mas nao abrem, e a lista sai vazia \
             exatamente como se o dispositivo nao estivesse conectado."
        );
    }

    // --- connect BEFORE grabbing -----------------------------------
    let connection = if observe {
        println!("  observe mode: does not grab and sends nothing");
        None
    } else {
        let mut addrs: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(&target).await?.collect();
        addrs.sort_by_key(|a| !a.is_ipv4());
        let dest =
            *addrs.first().ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
        let bind: std::net::SocketAddr =
            if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
        let ep = net::client(bind)?;
        // Failing here is the GOOD case: nobody has grabbed anything yet.
        let c = ep
            .connect(dest, "oxdesk-stream")?
            .await
            .map_err(|e| anyhow::anyhow!("could not connect to {target}: {e}. Nothing was grabbed."))?;
        println!("  connected to {target}");
        Some(c)
    };

    let send = match &connection {
        Some(c) => Some(c.open_uni().await?),
        None => None,
    };

    // --- grab -------------------------------------------------------
    if !observe && self_test == 0 {
        for d in &mut chosen {
            d.grab().map_err(|e| {
                anyhow::anyhow!("could not grab {}: {e}", d.name)
            })?;
        }
        println!("  grabbed; both Ctrl keys together give control back");
    }

    // --- read in threads, one per device ----------------------------
    //
    // One thread per device because `read` on evdev blocks, and the
    // keyboard cannot wait for the mouse to move.
    // TOKIO channel, not std.
    //
    // The first version used `std::sync::mpsc` and `recv_timeout` in the
    // main loop. `recv_timeout` BLOCKS the thread -- and in a
    // `current_thread` runtime the thread is the whole executor, including
    // the QUIC driver, which is what actually puts the packets on the
    // network.
    //
    // The result was the worst kind of bug: it seemed to work. Capture
    // read, `write_all` queued, and now and then a handful of events
    // escaped in the window where the driver got to run. The host on the
    // other side logged "input connected" and soon "input disconnected",
    // without a single event applied in between.
    //
    // The device threads are still real threads (`read` on evdev really
    // blocks) and talk to the loop through `blocking_send`, which is the
    // correct bridge between the two halves.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(1024);

    if self_test > 0 {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for _ in 0..self_test {
                if tx.blocking_send(Event::MouseRel { dx: 10.0, dy: 0.0 }).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        });
        println!("  self-test: {self_test} movements of +10 in X, without grabbing");
    }

    for mut d in chosen {
        if self_test > 0 {
            break; // do not read a real device in the self-test
        }
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match d.read() {
                Ok(events) => {
                    for e in events {
                        if tx.blocking_send(e).is_err() {
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

    // --- forward ----------------------------------------------------
    let mut buf = BytesMut::new();
    let mut pressed_keys: HashSet<u32> = HashSet::new();
    let mut send = send;
    // The reasons and the final line below stay pt-BR: they can be the last
    // line of the log when the process exits within oxdesk's 500 ms check.
    let reason;

    loop {
        let e = match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(e)) => e,
            // Every device thread died.
            Ok(None) => {
                reason = "os dispositivos pararam de responder".into();
                break;
            }
            Err(_) => {
                if let Some(c) = &connection {
                    if let Some(r) = c.close_reason() {
                        reason = format!("conexao caiu: {r}");
                        break;
                    }
                }
                continue;
            }
        };

        // Stuck-key bookkeeping. It has to apply ALSO in `--observar`,
        // otherwise the first real use starts with the set lying.
        if let Event::Key { keycode, pressed } = e {
            if pressed {
                pressed_keys.insert(keycode);
            } else {
                pressed_keys.remove(&keycode);
            }
        }

        if is_rescue(&pressed_keys) {
            reason = "socorro: os dois Ctrl".into();
            break;
        }

        if observe {
            println!("  {e:?}");
            continue;
        }

        buf.clear();
        e.write(&mut buf);
        if let Some(s) = &mut send {
            if let Err(err) = s.write_all(&buf).await {
                reason = format!("write failed: {err}");
                break;
            }
        }
    }

    // --- release what was left pressed on the other side ------------
    //
    // Without this, exiting with Shift pressed leaves the other machine
    // typing in uppercase forever. It already happened on the viewer path
    // (Mac -> Ubuntu) and cost a session restart.
    if let Some(s) = &mut send {
        for k in &pressed_keys {
            buf.clear();
            Event::Key { keycode: *k, pressed: false }.write(&mut buf);
            let _ = s.write_all(&buf).await;
        }
        let _ = s.finish();
    }
    // One turn so the last bytes go out before the socket closes.
    tokio::time::sleep(Duration::from_millis(120)).await;

    println!("  {reason}; {} tecla(s) soltas; controle de volta", pressed_keys.len());
    Ok(())
}
