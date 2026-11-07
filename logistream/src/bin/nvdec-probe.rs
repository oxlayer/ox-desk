//! Decodes an H.264 file through NVDEC and reports what came out.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! Exists to prove the decoding path WITHOUT a window, without the
//! network and without the viewer in between: if this binary decodes the
//! file that `logistream-sink` recorded, NVDEC is right and any remaining
//! problem belongs to whoever draws.
//!
//! Writes the last frame as PPM so it can be compared with the same frame
//! decoded by ffmpeg -- and so check the color matrix, which goes wrong
//! without raising an error.
//!
//!   logistream-probe-nvdec <file.h264> [output.ppm]

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    use std::io::Write;
    use std::time::Instant;

    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = a.first().cloned().unwrap_or_else(|| "/tmp/logistream-sink.h264".into());
    let ppm = a.get(1).cloned();

    let bytes = std::fs::read(&path)?;
    println!("  {} ({} KB)", path, bytes.len() / 1024);

    let mut dec = logistream::decode_nv::Decoder::new()
        .map_err(|e| anyhow::anyhow!("NVDEC unavailable: {e}"))?;
    println!("  NVDEC loaded");

    // In chunks, as it would come from the network: feeding the whole
    // file at once would work here and hide the real case.
    const CHUNK: usize = 64 * 1024;
    let t0 = Instant::now();
    let mut n = 0u64;
    let mut last = None;
    for part in bytes.chunks(CHUNK) {
        for q in dec.feed(part).map_err(|e| anyhow::anyhow!(e))? {
            n += 1;
            last = Some(q);
        }
    }
    let s = t0.elapsed().as_secs_f64();
    let (w, h) = dec.size();
    println!(
        "  {n} frames {w}x{h} in {s:.2}s = {:.0} fps  ({:.2} ms per frame)",
        n as f64 / s,
        s * 1000.0 / n.max(1) as f64
    );

    let Some(q) = last else {
        anyhow::bail!("no frame decoded");
    };

    // Statistics instead of "looks right": the per-channel mean catches a
    // black image, an inverted one or the wrong color matrix without
    // needing an eye.
    let mut rgb = vec![0u32; (q.w * q.h) as usize];
    logistream::decode_nv::nv12_to_bgra(&q, &mut rgb, q.w, q.h);
    let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
    for p in &rgb {
        sr += ((p >> 16) & 0xff) as u64;
        sg += ((p >> 8) & 0xff) as u64;
        sb += (p & 0xff) as u64;
    }
    let np = rgb.len() as u64;
    println!("  mean R={} G={} B={}", sr / np, sg / np, sb / np);

    if let Some(p) = ppm {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&p)?);
        write!(f, "P6\n{} {}\n255\n", q.w, q.h)?;
        for px in &rgb {
            f.write_all(&[((px >> 16) & 0xff) as u8, ((px >> 8) & 0xff) as u8, (*px & 0xff) as u8])?;
        }
        println!("  last frame in {p}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn main() {
    eprintln!("no NVDEC on macOS: here the viewer decodes through ffmpeg");
}
