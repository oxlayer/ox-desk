// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sends a few input events to the host and exits.
//!
//! It exists to prove the return path without depending on someone moving
//! the mouse in a window. The host logs every accepted event and shouts on
//! a refused one, so its log is the proof.
//!
//! **It really moves the target machine's cursor.** The movement is small
//! and the key is a lone Shift, which types nothing.
//!
//! With `--burst N` it sends N mouse movements with NO pause between them.
//! It serves to measure what the user felt: "the video stops if I stop
//! typing" and "input is slow" had the same cause -- injection waited for
//! Mutter's reply on the same thread that pumps the video. A burst while
//! `sink` measures fps shows the damage, and shows the improvement.
//!
//!   cargo run --release --bin input-send -- <host:port> [--burst N]

use bytes::BytesMut;
use oxdesk_stream::input::{Event, BTN_LEFT};
use oxdesk_stream::net;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let target = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9777".into());
    let mut addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&target).await?.collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    let dest = *addrs.first().ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
    let bind: std::net::SocketAddr =
        if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::client(bind)?;
    let conn = ep.connect(dest, "oxdesk-stream")?.await?;
    let mut send = conn.open_uni().await?;
    println!("  connected; sending events");

    let burst: usize = std::env::args()
        .position(|a| a == "--burst")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // `--rel N`: N relative movements of +10 in X.
    //
    // It exists to measure the KVM path without grabbing anyone's keyboard.
    // "Looks like it did not move" is not a measurement: with the cursor
    // position read before and after, it either moved 10*N pixels or it
    // did not.
    let rel: usize = std::env::args()
        .position(|a| a == "--rel")
        .and_then(|i| std::env::args().nth(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if rel > 0 {
        let mut buf = BytesMut::new();
        for _ in 0..rel {
            buf.clear();
            Event::MouseRel { dx: 10.0, dy: 0.0 }.write(&mut buf);
            send.write_all(&buf).await?;
            tokio::time::sleep(std::time::Duration::from_millis(8)).await;
        }
        send.finish()?;
        println!("  {rel} MouseRel of +10 in X = {} px expected", rel * 10);
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    // `--abs X Y`: ABSOLUTE position, in frame pixels.
    //
    // It is the path the viewer uses when you move the mouse inside the
    // window, and it was the only one broken on the Mac: the frame comes in
    // pixels and CGEvent injects in points, a factor of 2 on a Retina.
    // `--rel` does not catch it -- it accumulates deltas and never goes
    // through the conversion. I measured "exactly +200 px" with the bug
    // still in place.
    //
    // To check, send the MIDDLE of the frame: the cursor has to stop in the
    // middle of the screen. With the bug it goes to the bottom-right
    // corner, because double of everything is bigger than the screen and
    // the system clamps to the edge.
    if let Some(i) = std::env::args().position(|a| a == "--abs") {
        let x: f64 = std::env::args().nth(i + 1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let y: f64 = std::env::args().nth(i + 2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let mut buf = BytesMut::new();
        Event::Mouse { x, y }.write(&mut buf);
        send.write_all(&buf).await?;
        send.finish()?;
        println!("  absolute Mouse to ({x}, {y}) in frame pixels");
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    // `--leave-held <evdev>`: presses the key and leaves WITHOUT releasing
    // it -- a source that crashed mid-press. The host must release it on
    // its own when the stream ends (`input::Held`); its log says
    // "released 1 key(s)/button(s) the client left held". Use a key that
    // types nothing alone (42 = left Shift).
    if let Some(i) = std::env::args().position(|a| a == "--leave-held") {
        let key: u32 = std::env::args().nth(i + 1).and_then(|v| v.parse().ok()).unwrap_or(42);
        let mut buf = BytesMut::new();
        Event::Key { keycode: key, pressed: true }.write(&mut buf);
        send.write_all(&buf).await?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        send.finish()?;
        println!("  key {key} pressed and left held");
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    // `--capslock`: turns the target machine's Caps Lock on and off.
    //
    // The keyboard needed a MEASURABLE test. "I typed and nothing showed
    // up" depends on where the focus was; the Caps Lock state depends on
    // nothing and is read with one call. Harmless: it goes back to how it
    // was.
    if std::env::args().any(|a| a == "--capslock") {
        let mut buf = BytesMut::new();
        // 58 = evdev KEY_CAPSLOCK, which is what the protocol carries.
        for pressed in [true, false] {
            buf.clear();
            Event::Key { keycode: 58, pressed }.write(&mut buf);
            send.write_all(&buf).await?;
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        send.finish()?;
        println!("  caps lock toggled once");
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    // `--hold <evdev>`: presses the key, waits, and releases it.
    //
    // It exists because Caps Lock does NOT work as a test on macOS: the
    // system handles it in the HID layer and a synthetic event does not
    // toggle it, even when it arrives and gets translated. With a common
    // key you can look at its state while it is held, which is a real
    // measurement.
    if let Some(i) = std::env::args().position(|a| a == "--hold") {
        let code: u32 = std::env::args()
            .nth(i + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(30); // 30 = A
        let mut buf = BytesMut::new();
        buf.clear();
        Event::Key { keycode: code, pressed: true }.write(&mut buf);
        send.write_all(&buf).await?;
        println!("  key {code} PRESSED; holding 4s");
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        buf.clear();
        Event::Key { keycode: code, pressed: false }.write(&mut buf);
        send.write_all(&buf).await?;
        send.finish()?;
        println!("  key {code} released");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return Ok(());
    }

    if burst > 0 {
        // Movement only, no key or button: it changes the target machine's
        // cursor and nothing else. A small circle so the cursor does not
        // stop in a corner that triggers something in the system.
        let mut buf = BytesMut::new();
        let t0 = std::time::Instant::now();
        for i in 0..burst {
            let a = (i as f64) * 0.1;
            let e = Event::Mouse { x: 1300.0 + 60.0 * a.cos(), y: 750.0 + 60.0 * a.sin() };
            buf.clear();
            e.write(&mut buf);
            send.write_all(&buf).await?;
        }
        send.finish()?;
        let s = t0.elapsed().as_secs_f64();
        println!("  {burst} events in {s:.2}s = {:.0}/s", burst as f64 / s);
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        return Ok(());
    }

    let mut events = Vec::new();
    // A small square near the center: visible, without crossing the screen
    // or landing in a corner that triggers something in the system.
    for (x, y) in [(1200.0, 700.0), (1400.0, 700.0), (1400.0, 800.0), (1200.0, 800.0)] {
        events.push(Event::Mouse { x, y });
    }
    // Shift: press and release without typing anything.
    events.push(Event::Key { keycode: 42, pressed: true });
    events.push(Event::Key { keycode: 42, pressed: false });
    // A one-step scroll, which destroys nothing.
    events.push(Event::Scroll { dx: 0.0, dy: 15.0 });
    // A button is NOT sent: clicking blindly on someone's machine could
    // press whatever is under the cursor.
    let _ = BTN_LEFT;

    let mut buf = BytesMut::new();
    for e in &events {
        buf.clear();
        e.write(&mut buf);
        send.write_all(&buf).await?;
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
    send.finish()?;
    // Let the host consume before closing the connection.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    println!("  {} events sent", events.len());
    Ok(())
}
