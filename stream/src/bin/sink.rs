//! Receiver: QUIC -> frames -> file. v2 on the far side.
//!
//! It does not draw anything yet -- decoding and rendering are the next
//! step. What this binary answers is whether the **transport** holds up:
//! how many frames arrive whole, how many are lost, and with what delay.
//!
//! The clock is not the same on both machines, so the latency only means
//! something on loopback. Between machines the number becomes the clock
//! skew plus the network, and it is labeled that way instead of being
//! presented as truth.
//!
//! With `--rajada N` (burst) it also sends N mouse movements over the
//! SAME connection, as the viewer does. That matters: the host serves one
//! client at a time, so measuring input and video over separate
//! connections reproduces nothing -- the second one waits in the queue.
//! That is how the first attempt to measure "the video stops when I type"
//! measured nothing.
//!
//!   cargo run --release --bin sink -- <host:port> [seconds] [--rajada N]

use std::io::Write;
use std::time::{Duration, Instant};

use oxdesk_stream::net::{self, Reassembler};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let target = a.first().cloned().unwrap_or_else(|| "127.0.0.1:9777".into());
    let seconds: u64 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(10);

    // Resolve ALL addresses and prefer IPv4, rather than taking the first.
    //
    // `linux-pc.local` resolves in both families, and the Mac returns
    // IPv6 first. With the socket bound to `0.0.0.0`, quinn refuses with
    // "invalid remote address" -- the same trap the daemon's `ssh` had
    // already hit. The local socket has to be born in the destination's
    // family.
    let mut addrs: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host(&target).await?.collect();
    addrs.sort_by_key(|a| !a.is_ipv4());
    let dest = *addrs
        .first()
        .ok_or_else(|| anyhow::anyhow!("could not resolve {target}"))?;
    let bind: std::net::SocketAddr =
        if dest.is_ipv4() { "0.0.0.0:0".parse()? } else { "[::]:0".parse()? };
    let ep = net::client(bind)?;
    println!("  connecting to {dest}");
    let conn = ep.connect(dest, "oxdesk-stream")?.await?;
    println!("  connected");

    // Input burst over the SAME connection, before measuring starts.
    let burst: usize = a
        .iter()
        .position(|x| x == "--rajada")
        .and_then(|i| a.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if burst > 0 {
        use bytes::BytesMut;
        use oxdesk_stream::input::Event;
        let mut send = conn.open_uni().await?;
        let mut buf = BytesMut::new();
        for i in 0..burst {
            let ang = (i as f64) * 0.1;
            let e = Event::Mouse {
                x: 1300.0 + 60.0 * ang.cos(),
                y: 750.0 + 60.0 * ang.sin(),
            };
            buf.clear();
            e.write(&mut buf);
            send.write_all(&buf).await?;
        }
        println!("  {burst} input events sent over the same connection");
    }

    let path = std::env::temp_dir().join("oxdesk-sink.h264");
    let mut file = std::fs::File::create(&path)?;

    let mut r = Reassembler::default();
    let mut frames = 0u64;
    let mut bytes = 0u64;
    let mut datagrams = 0u64;
    let mut delays_ms: Vec<f64> = Vec::new();
    let t0 = Instant::now();
    let end = t0 + Duration::from_secs(seconds);

    while Instant::now() < end {
        let remaining = end.saturating_duration_since(Instant::now());
        let d = match tokio::time::timeout(remaining, conn.read_datagram()).await {
            Ok(Ok(d)) => d,
            Ok(Err(e)) => {
                println!("  connection closed: {e}");
                break;
            }
            Err(_) => break,
        };
        datagrams += 1;
        bytes += d.len() as u64;
        if let Some((hdr, frame)) = r.push(&d) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.as_nanos() as u64)
                .unwrap_or(0);
            delays_ms.push((now.saturating_sub(hdr.captured_at_ns)) as f64 / 1e6);
            file.write_all(&frame)?;
            frames += 1;
        }
    }
    file.flush()?;

    let s = t0.elapsed().as_secs_f64();
    let pct = |v: &Vec<f64>, q: usize| {
        let mut x = v.clone();
        x.sort_by(|a, b| a.partial_cmp(b).unwrap());
        x.get(x.len().saturating_mul(q) / 100).copied().unwrap_or(0.0)
    };
    println!("\n  {frames} whole frames in {s:.1}s = {:.1} fps", frames as f64 / s.max(0.001));
    println!("  {datagrams} datagrams, {:.1} Mbps", bytes as f64 * 8.0 / s / 1_000_000.0);
    println!("  frames lost (arrived incomplete): {}", r.lost);
    if !delays_ms.is_empty() {
        println!(
            "  capture -> frame assembled: p50 {:.1}ms  p95 {:.1}ms  (only valid on loopback: \
             between machines this includes the clock skew)",
            pct(&delays_ms, 50),
            pct(&delays_ms, 95)
        );
    }
    println!("  {}", path.display());
    Ok(())
}
