//! Kills the biggest unknown of v1: does NVENC work on this machine, loaded
//! at runtime, without the SDK and without sudo?
//!
//! Encodes SYNTHETIC frames -- capture is a different problem, and mixing the
//! two would make the failure of one look like the other's. If this binary
//! writes a .h264 that ffprobe reads, encoding is proven and the only unknown
//! left in v1 is capture.
//!
//!   cargo run --release --bin nvenc-probe -- [frames] [width] [height]

use std::io::Write;
use std::time::Instant;

use cudarc::driver::CudaContext;
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, NV_ENC_TUNING_INFO,
};
use moq_nvenc::{EncodePictureParams, Encoder, EncoderInitParams};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let n: u64 = a.first().and_then(|v| v.parse().ok()).unwrap_or(120);
    let w: u32 = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(1920);
    let h: u32 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(1080);

    // `load` before touching CUDA: the crate separates "the library exists
    // and is new enough" from "the GPU accepted", and knowing which of the
    // two failed matters on a machine without a driver.
    Encoder::load().map_err(|e| anyhow::anyhow!("NVENC unavailable: {e:?}"))?;
    println!("  NVENC loaded (dlopen of libnvidia-encode)");

    let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA context: {e:?}"))?;
    let enc = Encoder::initialize_with_cuda(ctx)
        .map_err(|e| anyhow::anyhow!("NVENC session: {e:?}"))?;

    let codecs = enc.get_encode_guids().map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("  codecs offered by the GPU: {}", codecs.len());
    anyhow::ensure!(
        codecs.contains(&NV_ENC_CODEC_H264_GUID),
        "this GPU does not offer hardware H.264"
    );

    // Without an explicit config the bitstream comes out WITHOUT SPS/PPS,
    // and ffprobe refuses it with "non-existing PPS 0 referenced". This is
    // not a file detail: a client joining mid-stream -- the normal case for
    // a remote screen -- needs the headers to start drawing. `repeatSPSPPS`
    // resends them on every IDR.
    let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
    let preset = enc
        .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
        .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
    let mut cfg = preset.presetCfg;
    // bindgen union: only the chosen codec is valid in there, and we chose
    // H.264 on the line above.
    unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

    let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
    params
        .preset_guid(NV_ENC_PRESET_P4_GUID)
        // A remote screen lives on latency, not on pretty bitrate.
        .tuning_info(tuning)
        .framerate(60, 1)
        .enable_picture_type_decision();
    // Safe: the config came from `get_preset_config` itself, whose reserved
    // pointers are null, as the crate docs require.
    unsafe { params.encode_config(cfg) };

    let session = enc
        .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
        .map_err(|e| anyhow::anyhow!("start session {w}x{h}: {e}"))?;

    let mut input = session.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut output = session.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;

    let dest = std::env::temp_dir().join("oxdesk-probe.h264");
    let mut file = std::fs::File::create(&dest)?;

    let mut frame = vec![0u8; (w * h * 4) as usize];
    let mut bytes = 0usize;
    let mut timings = Vec::new();
    let t0 = Instant::now();

    for i in 0..n {
        // A moving gradient: static content would make the encoder produce
        // nearly empty frames and the number would measure nothing.
        paint(&mut frame, w, h, i);

        let tq = Instant::now();
        {
            let mut lock = input.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
            // Safe here: the buffer was created with these same dimensions
            // and format, so the size matches by construction.
            unsafe { lock.write(&frame) };
        }
        let sub = session
            .encode_picture(
                input,
                output,
                EncodePictureParams { input_timestamp: i, force_idr: i == 0 },
            )
            .map_err(|e| anyhow::anyhow!("encode of frame {i}: {e}"))?;
        let (data, e2, s2) = sub.finish().map_err(|e| anyhow::anyhow!("{e}"))?;
        timings.push(tq.elapsed().as_secs_f64() * 1000.0);
        bytes += data.len();
        file.write_all(&data)?;
        input = e2;
        output = s2;
    }
    file.flush()?;

    let total = t0.elapsed().as_secs_f64();
    timings.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = timings[timings.len() / 2];
    let p95 = timings[timings.len() * 95 / 100];
    println!("  {n} frames {w}x{h} in {total:.2}s = {:.0} fps", n as f64 / total);
    println!("  per frame: p50 {p50:.2}ms  p95 {p95:.2}ms");
    println!(
        "  {:.1} KB total = {:.1} Mbps at 60fps",
        bytes as f64 / 1024.0,
        (bytes as f64 * 8.0 / n as f64) * 60.0 / 1_000_000.0
    );
    println!("  written to {}", dest.display());
    Ok(())
}

/// Diagonal gradient that slides with the frame.
fn paint(buf: &mut [u8], w: u32, h: u32, i: u64) {
    let offset = (i * 7) as u32;
    for y in 0..h {
        let row = (y * w * 4) as usize;
        for x in 0..w {
            let p = row + (x * 4) as usize;
            buf[p] = ((x + offset) % 256) as u8;
            buf[p + 1] = ((y + offset) % 256) as u8;
            buf[p + 2] = ((x + y + offset) % 256) as u8;
            buf[p + 3] = 255;
        }
    }
}
