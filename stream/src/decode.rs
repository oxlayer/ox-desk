//! Decoding, with the platform picking the path.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! | where | how | why |
//! |---|---|---|
//! | macOS | `ffmpeg` (libavcodec from brew) | it has no NVIDIA |
//! | Linux, Windows | NVDEC | the GPU is right there and the decoder ships with the driver |
//!
//! **One per platform, not both everywhere.** Loading both would mean
//! `libavcodec` installed on Windows -- which is exactly what kept that
//! side on RDP. And two active paths on the same machine would multiply
//! the failure modes with no one gaining anything: the Mac cannot use
//! NVDEC and the other two do not need ffmpeg.
//!
//! The boundary is this: an H.264 access unit goes in, pixels ready for
//! the window come out, ALREADY at the size and position where they will
//! be drawn. The window decides that size, so it passes the math in as a
//! function -- the decoder is the only one that knows the source size,
//! and without it the math does not work out.

/// A frame ready for the viewer's `Slot`.
pub struct DecodedFrame {
    pub rgb: Vec<u32>,
    /// Corner where the image starts inside the window.
    pub ox: u32,
    pub oy: u32,
    /// Drawn size.
    pub w: u32,
    pub h: u32,
    /// Remote screen size -- the space the host speaks in.
    pub rw: u32,
    pub rh: u32,
}

/// Given the SOURCE size, returns `(ox, oy, w, h)` of the destination.
pub type Fit<'a> = &'a mut dyn FnMut(u32, u32) -> (u32, u32, u32, u32);

#[cfg(target_os = "macos")]
mod inner {
    use super::{Fit, DecodedFrame};

    /// The Mac path: libavcodec decodes, swscale converts and scales.
    pub struct Decoder {
        dec: ffmpeg_next::decoder::Video,
        scaler: Option<ffmpeg_next::software::scaling::Context>,
        /// (source_w, source_h, dest_w, dest_h) of the current scaler.
        /// Recreating it is expensive; only when it actually changed.
        key: (u32, u32, u32, u32),
    }

    impl Decoder {
        pub fn new() -> Result<Self, String> {
            ffmpeg_next::init().map_err(|e| format!("ffmpeg: {e}"))?;
            let codec = ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264)
                .ok_or("no H.264 decoder")?;
            let ctx = ffmpeg_next::codec::context::Context::new_with_codec(codec);
            let dec = ctx.decoder().video().map_err(|e| format!("decoder: {e}"))?;
            Ok(Decoder { dec, scaler: None, key: (0, 0, 0, 0) })
        }

        pub fn name(&self) -> &'static str {
            "ffmpeg"
        }

        pub fn feed(&mut self, bytes: &[u8], fit: Fit) -> Result<Vec<DecodedFrame>, String> {
            let packet = ffmpeg_next::Packet::copy(bytes);
            if self.dec.send_packet(&packet).is_err() {
                // Frame without a reference (joined mid-stream): not an
                // error, it just yields nothing. The next keyframe fixes it.
                return Ok(Vec::new());
            }
            let mut out = Vec::new();
            let mut frame = ffmpeg_next::frame::Video::empty();
            while self.dec.receive_frame(&mut frame).is_ok() {
                let (rw, rh) = (frame.width(), frame.height());
                let (ox, oy, dw, dh) = fit(rw, rh);
                if dw == 0 || dh == 0 {
                    continue;
                }
                if self.scaler.is_none() || self.key != (rw, rh, dw, dh) {
                    self.scaler = ffmpeg_next::software::scaling::Context::get(
                        frame.format(),
                        rw,
                        rh,
                        ffmpeg_next::format::Pixel::BGRA,
                        dw,
                        dh,
                        ffmpeg_next::software::scaling::Flags::BILINEAR,
                    )
                    .ok();
                    self.key = (rw, rh, dw, dh);
                }
                let Some(sc) = self.scaler.as_mut() else { continue };
                let mut bgra = ffmpeg_next::frame::Video::empty();
                if sc.run(&frame, &mut bgra).is_err() {
                    continue;
                }
                // Copy per ROW, not per pixel.
                //
                // It was a per-pixel loop with a bounds check inside:
                // 921 thousand iterations per frame in a 1280x720 window,
                // at 60-80 frames per second, to produce bytes that
                // `swscale` had ALREADY delivered in the right layout. BGRA
                // in bytes is exactly `0xAARRGGBB` in little-endian, which
                // is what softbuffer expects.
                //
                // So it is 720 `copy_from_slice` calls instead of 921
                // thousand conversions. The `unsafe` reinterprets `[u32]`
                // as `[u8]`: same total size, and going from alignment 4
                // to 1 is always valid.
                //
                // Little-endian is assumed here and on both ends we care
                // about (x86-64 and Apple Silicon). On a big-endian target
                // this would swap the channels -- loud enough for someone
                // to notice on the first image.
                let stride = bgra.stride(0);
                let data = bgra.data(0);
                let row_bytes = dw as usize * 4;
                let mut rgb = vec![0u32; (dw * dh) as usize];
                {
                    let dest: &mut [u8] = unsafe {
                        std::slice::from_raw_parts_mut(
                            rgb.as_mut_ptr() as *mut u8,
                            rgb.len() * 4,
                        )
                    };
                    for y in 0..dh as usize {
                        let o = y * stride;
                        let d = y * row_bytes;
                        if o + row_bytes > data.len() || d + row_bytes > dest.len() {
                            break;
                        }
                        dest[d..d + row_bytes]
                            .copy_from_slice(&data[o..o + row_bytes]);
                    }
                }
                out.push(DecodedFrame { rgb, ox, oy, w: dw, h: dh, rw, rh });
            }
            Ok(out)
        }
    }
}

#[cfg(any(target_os = "linux", windows))]
mod inner {
    use super::{Fit, DecodedFrame};
    use crate::decode_nv;

    /// The path for the NVIDIA machines: NVDEC decodes, and the color
    /// conversion scales in the same pass.
    pub struct Decoder {
        dec: decode_nv::Decoder,
    }

    impl Decoder {
        pub fn new() -> Result<Self, String> {
            Ok(Decoder { dec: decode_nv::Decoder::new()? })
        }

        pub fn name(&self) -> &'static str {
            "nvdec"
        }

        pub fn feed(&mut self, bytes: &[u8], fit: Fit) -> Result<Vec<DecodedFrame>, String> {
            let mut out = Vec::new();
            for q in self.dec.feed(bytes)? {
                let (ox, oy, dw, dh) = fit(q.w, q.h);
                if dw == 0 || dh == 0 {
                    continue;
                }
                let mut rgb = vec![0u32; (dw * dh) as usize];
                decode_nv::nv12_to_bgra(&q, &mut rgb, dw, dh);
                out.push(DecodedFrame { rgb, ox, oy, w: dw, h: dh, rw: q.w, rh: q.h });
            }
            Ok(out)
        }
    }
}

pub use inner::Decoder;
