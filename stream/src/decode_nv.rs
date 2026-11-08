//! NVDEC decoding: H.264 -> NV12 -> BGRA, on the GPU.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why it exists, when `ffmpeg-next` already decodes
//!
//! Because `libavcodec` has to be installed, and that blocked the last
//! path still using RDP: there is no `libavcodec` on Windows, and shipping
//! the DLLs there (or setting up cross-compilation against the FFmpeg
//! headers) is plumbing that improves nothing. The two machines that
//! watch each other have NVIDIA GPUs, and the decoder already ships with
//! the driver -- the same `moq-nvenc` we use to encode exposes the NVDEC
//! entry points, loaded at runtime.
//!
//! The Mac stays on `ffmpeg`: it has no NVIDIA. So there are two decoding
//! paths on purpose, chosen per platform, rather than one crooked path
//! that serves both poorly.
//!
//! ## How CUVID wants to be used
//!
//! It is not "give me a packet, return a frame". It is a parser with three
//! C callbacks, called from inside `cuvidParseVideoData`:
//!
//! ```text
//!   sequence -> the stream introduced itself; create the decoder
//!   decode   -> a frame is ready to decode; submit it
//!   display  -> a frame is ready to show; map and copy it
//! ```
//!
//! That is why the state lives in a `Box` whose pointer goes as
//! `pUserData`: the callbacks are `extern "C"` and capture nothing.

use std::ffi::c_void;
use std::sync::Arc;

use cudarc::driver::CudaContext;
use moq_nvenc::cuvid;
use moq_nvenc::sys::cuviddec::{
    cudaVideoChromaFormat_enum, cudaVideoCodec_enum, cudaVideoCreateFlags_enum,
    cudaVideoDeinterlaceMode_enum, cudaVideoSurfaceFormat_enum, CUvideodecoder,
    CUVIDDECODECREATEINFO, CUVIDPICPARAMS, CUVIDPROCPARAMS,
};
use moq_nvenc::sys::nvcuvid::{
    CUvideoparser, CUVIDEOFORMAT, CUVIDPARSERDISPINFO, CUVIDPARSERPARAMS, CUVIDSOURCEDATAPACKET,
};

/// A decoded frame, still in NV12, already in CPU memory.
pub struct Nv12 {
    pub data: Vec<u8>,
    /// Row pitch. NVDEC aligns it, so it is almost never equal to the width.
    pub pitch: usize,
    pub w: u32,
    pub h: u32,
}

/// What the C callbacks share with the owner.
///
/// Not `Send`: the CUDA context is per thread, and the callbacks are
/// called from inside `parse` -- on the same thread, always.
struct State {
    decoder: CUvideodecoder,
    /// CODED size (multiple of 16); the display size comes from
    /// `display_area` and is what matters to the user.
    w: u32,
    h: u32,
    /// Ready frames. Short queue: an old frame on a remote screen has no
    /// value, so the owner drains it on every pass and the rest is lost.
    ready: Vec<Nv12>,
    /// Error from inside the callback, for the owner to see. A C callback
    /// cannot propagate a `Result` nor unwind a panic.
    error: Option<String>,
}

pub struct Decoder {
    parser: CUvideoparser,
    state: Box<State>,
    /// Keeps the context alive while the decoder exists.
    _ctx: Arc<CudaContext>,
}

/// A CUDA/CUVID error becomes text with the name of the call that failed --
/// "cuvidCreateDecoder: 801" says where to look; "error 801" does not.
fn check(name: &str, r: cudarc::driver::sys::CUresult) -> Result<(), String> {
    if r == cudarc::driver::sys::CUresult::CUDA_SUCCESS {
        Ok(())
    } else {
        Err(format!("{name}: {r:?}"))
    }
}

impl Decoder {
    pub fn new() -> Result<Self, String> {
        let api = cuvid::Api::get()?;
        let ctx = CudaContext::new(0).map_err(|e| format!("CUDA: {e:?}"))?;

        let mut state = Box::new(State {
            decoder: std::ptr::null_mut(),
            w: 0,
            h: 0,
            ready: Vec::new(),
            error: None,
        });

        let mut p: CUVIDPARSERPARAMS = unsafe { std::mem::zeroed() };
        p.CodecType = cudaVideoCodec_enum::cudaVideoCodec_H264;
        // Ceiling on decode surfaces. The sequence callback returns the
        // minimum the stream requires; this number only needs to be
        // >= that.
        p.ulMaxNumDecodeSurfaces = 8;
        // Zero: we want the frame as soon as it comes out, without
        // reordering for presentation. A remote screen has no B-frames
        // (the host turns them off) and display delay is exactly what we
        // do not want.
        p.ulMaxDisplayDelay = 0;
        p.pUserData = (&mut *state) as *mut State as *mut c_void;
        p.pfnSequenceCallback = Some(cb_sequence);
        p.pfnDecodePicture = Some(cb_decode);
        p.pfnDisplayPicture = Some(cb_display);

        let mut parser: CUvideoparser = std::ptr::null_mut();
        check("cuvidCreateVideoParser", unsafe {
            (api.create_video_parser)(&mut parser, &mut p)
        })?;

        Ok(Decoder { parser, state, _ctx: ctx })
    }

    /// Frame size, zero before the first sequence header.
    pub fn size(&self) -> (u32, u32) {
        (self.state.w, self.state.h)
    }

    /// Feeds stream bytes (Annex-B) and returns whatever became ready.
    ///
    /// The callbacks run INSIDE this call, so frames show up before it
    /// returns -- there is nothing to wait for afterwards.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Nv12>, String> {
        let api = cuvid::Api::get()?;
        let mut pkt: CUVIDSOURCEDATAPACKET = unsafe { std::mem::zeroed() };
        pkt.payload = bytes.as_ptr();
        pkt.payload_size = bytes.len() as _;
        check("cuvidParseVideoData", unsafe {
            (api.parse_video_data)(self.parser, &mut pkt)
        })?;
        if let Some(e) = self.state.error.take() {
            return Err(e);
        }
        Ok(std::mem::take(&mut self.state.ready))
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // Order matters: the parser calls the callbacks, which use the
        // decoder. First stop the calls, then destroy.
        if let Ok(api) = cuvid::Api::get() {
            unsafe {
                if !self.parser.is_null() {
                    let _ = (api.destroy_video_parser)(self.parser);
                }
                if !self.state.decoder.is_null() {
                    let _ = (api.destroy_decoder)(self.state.decoder);
                }
            }
        }
    }
}

/// Turns the user pointer back into `&mut State`.
///
/// `unsafe` concentrated here: it is the only place where we trust that
/// the pointer CUVID hands back is the `Box` we gave it.
unsafe fn state_from(p: *mut c_void) -> Option<&'static mut State> {
    (!p.is_null()).then(|| &mut *(p as *mut State))
}

/// The stream introduced itself: create the decoder.
///
/// Returns the number of decode surfaces (>0) to continue, or 0 to abort.
/// Zero here means "cannot do it", and `parse` returns an error.
extern "C" fn cb_sequence(user: *mut c_void, fmt: *mut CUVIDEOFORMAT) -> i32 {
    let Some(st) = (unsafe { state_from(user) }) else { return 0 };
    let f = unsafe { &*fmt };

    // DISPLAY area, not the coded one: 1440 stays 1440 coded, but a
    // 1080 becomes 1088 coded and showing that would put 8 rows of
    // garbage at the bottom.
    let w = (f.display_area.right - f.display_area.left).max(0) as u32;
    let h = (f.display_area.bottom - f.display_area.top).max(0) as u32;
    if w == 0 || h == 0 {
        st.error = Some("sequence without a display area".into());
        return 0;
    }

    let api = match cuvid::Api::get() {
        Ok(a) => a,
        Err(e) => {
            st.error = Some(e.to_string());
            return 0;
        }
    };

    // Resolution changed mid-stream: recreate. This really happens --
    // the Windows host rebuilds the encoder when the monitor changes.
    if !st.decoder.is_null() {
        unsafe { (api.destroy_decoder)(st.decoder) };
        st.decoder = std::ptr::null_mut();
    }

    let mut ci: CUVIDDECODECREATEINFO = unsafe { std::mem::zeroed() };
    ci.ulWidth = f.coded_width as _;
    ci.ulHeight = f.coded_height as _;
    ci.ulMaxWidth = f.coded_width as _;
    ci.ulMaxHeight = f.coded_height as _;
    ci.CodecType = f.codec;
    ci.ChromaFormat = f.chroma_format;
    ci.bitDepthMinus8 = f.bit_depth_luma_minus8 as _;
    ci.OutputFormat = cudaVideoSurfaceFormat_enum::cudaVideoSurfaceFormat_NV12;
    // Weave: the stream is progressive (a computer screen). Bob or
    // Adaptive exist for interlaced video and would cost quality here.
    ci.DeinterlaceMode = cudaVideoDeinterlaceMode_enum::cudaVideoDeinterlaceMode_Weave;
    ci.ulCreationFlags = cudaVideoCreateFlags_enum::cudaVideoCreate_PreferCUVID as _;
    ci.ulNumDecodeSurfaces = f.min_num_decode_surfaces.max(2) as _;
    // One output surface: we map and copy right away, inside the
    // callback. More than one would only help someone holding several
    // frames.
    ci.ulNumOutputSurfaces = 1;
    ci.ulTargetWidth = w as _;
    ci.ulTargetHeight = h as _;
    ci.display_area.left = f.display_area.left as i16;
    ci.display_area.top = f.display_area.top as i16;
    ci.display_area.right = f.display_area.right as i16;
    ci.display_area.bottom = f.display_area.bottom as i16;

    let mut dec: CUvideodecoder = std::ptr::null_mut();
    let r = unsafe { (api.create_decoder)(&mut dec, &mut ci) };
    if let Err(e) = check("cuvidCreateDecoder", r) {
        st.error = Some(e);
        return 0;
    }
    st.decoder = dec;
    st.w = w;
    st.h = h;
    // 4:2:0 chroma means UV with half the rows; the rest of the code
    // assumes that when copying `h * 3 / 2` rows.
    if f.chroma_format != cudaVideoChromaFormat_enum::cudaVideoChromaFormat_420 {
        st.error = Some(format!("chroma {:?} not supported", f.chroma_format));
        return 0;
    }
    ci.ulNumDecodeSurfaces as i32
}

extern "C" fn cb_decode(user: *mut c_void, pic: *mut CUVIDPICPARAMS) -> i32 {
    let Some(st) = (unsafe { state_from(user) }) else { return 0 };
    if st.decoder.is_null() {
        return 0;
    }
    let Ok(api) = cuvid::Api::get() else { return 0 };
    let r = unsafe { (api.decode_picture)(st.decoder, pic) };
    match check("cuvidDecodePicture", r) {
        Ok(()) => 1,
        Err(e) => {
            st.error = Some(e);
            0
        }
    }
}

extern "C" fn cb_display(user: *mut c_void, info: *mut CUVIDPARSERDISPINFO) -> i32 {
    let Some(st) = (unsafe { state_from(user) }) else { return 0 };
    if st.decoder.is_null() {
        return 0;
    }
    // `catch_unwind`: unwinding a panic across an `extern "C"` boundary
    // is undefined behavior, and there is allocation in here (which can
    // fail). Better to turn it into our own error than corrupt the
    // driver's stack.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| copy_frame(st, info)));
    match r {
        Ok(Ok(())) => 1,
        Ok(Err(e)) => {
            st.error = Some(e);
            0
        }
        Err(_) => {
            st.error = Some("panic in the display callback".into());
            0
        }
    }
}

fn copy_frame(st: &mut State, info: *mut CUVIDPARSERDISPINFO) -> Result<(), String> {
    let api = cuvid::Api::get()?;
    let d = unsafe { &*info };

    let mut pp: CUVIDPROCPARAMS = unsafe { std::mem::zeroed() };
    pp.progressive_frame = d.progressive_frame;
    pp.top_field_first = d.top_field_first;

    let mut dptr: u64 = 0;
    let mut pitch: u32 = 0;
    check("cuvidMapVideoFrame", unsafe {
        (api.map_video_frame)(st.decoder, d.picture_index, &mut dptr, &mut pitch, &mut pp)
    })?;

    // Y (h rows) and UV (h/2 rows) are contiguous in the mapped surface,
    // with the SAME pitch -- hence one copy instead of two.
    let rows = (st.h as usize) * 3 / 2;
    let bytes = pitch as usize * rows;
    let mut data = vec![0u8; bytes];
    let copied = unsafe {
        cudarc::driver::sys::cuMemcpyDtoH_v2(
            data.as_mut_ptr() as *mut c_void,
            dptr as cudarc::driver::sys::CUdeviceptr,
            bytes,
        )
    };
    // ALWAYS unmap, even when the copy fails: a mapped surface that is
    // never returned stalls the decoder on the next frame.
    let _ = unsafe { (api.unmap_video_frame)(st.decoder, dptr) };
    check("cuMemcpyDtoH", copied)?;

    st.ready.push(Nv12 { data, pitch: pitch as usize, w: st.w, h: st.h });
    Ok(())
}

/// NV12 -> BGRA, resizing in the same pass.
///
/// One pass over the OUTPUT pixels, not the input: when shrinking (the
/// normal case, a window smaller than the remote screen) this does less
/// work than converting and then scaling.
///
/// **Bilinear sampling.** The first version used nearest neighbor, with a
/// comment saying bilinear was "the next step if it bothers anyone". It
/// did: the user compared Mac->Windows (which decodes through `swscale`,
/// bilinear) with Ubuntu->Windows (this path) and called the second one
/// awful. Shrinking 1440p text with nearest neighbor jags every letter --
/// and it was only on the GPU path, which made the difference jump out
/// side by side.
///
/// The mapping uses the pixel center (`(x+0.5)*scale-0.5`), not
/// `x*scale`: without the half pixel the image shifts half a pixel up and
/// to the left, and at 1:1 the interpolation would no longer be exact.
///
/// Limited-range BT.709: that is what a 1440p desktop is, and the encoder
/// does not write the color description into the stream. Checked against
/// ffmpeg on the same frame -- one point of difference in the green.
pub fn nv12_to_bgra(q: &Nv12, dst: &mut [u32], dw: u32, dh: u32) {
    if dw == 0 || dh == 0 || q.w == 0 || q.h == 0 {
        return;
    }
    let (sw, sh) = (q.w as usize, q.h as usize);
    let uv_plane = q.pitch * sh;
    let ex = sw as f32 / dw as f32;
    let ey = sh as f32 / dh as f32;

    // The source column does not depend on the row: the same 1451
    // floating-point computations were being redone 1232 times per frame.
    // One table per frame trades that for three vector reads.
    let columns: Vec<(usize, usize, i32)> = (0..dw as usize)
        .map(|x| {
            let fx = ((x as f32 + 0.5) * ex - 0.5).max(0.0);
            let x0 = (fx as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            (x0, x1, ((fx - x0 as f32) * 256.0) as i32)
        })
        .collect();

    // An output row does not look at any other, so splitting the frame
    // into bands is exact, not approximate. Without this the conversion
    // alone cost 28 ms (a ceiling of 35 fps) and held drawing at 27 while
    // the network was already delivering 60.
    let slices = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
        .min(dh as usize);
    let per_slice = (dh as usize).div_ceil(slices);
    let data = &q.data;
    let pitch = q.pitch;
    let columns = &columns;

    std::thread::scope(|scope| {
        for (n, band) in dst.chunks_mut(per_slice * dw as usize).enumerate() {
            let y_start = n * per_slice;
            scope.spawn(move || {
                for (dy, row) in band.chunks_mut(dw as usize).enumerate() {
                    let y = y_start + dy;
                    let fy = ((y as f32 + 0.5) * ey - 0.5).max(0.0);
                    let y0 = (fy as usize).min(sh - 1);
                    let y1 = (y0 + 1).min(sh - 1);
                    let wy = ((fy - y0 as f32) * 256.0) as i32;
                    let l0 = y0 * pitch;
                    let l1 = y1 * pitch;
                    let uv_row = uv_plane + (y0 / 2) * pitch;

                    for (x, px) in row.iter_mut().enumerate() {
                        let (x0, x1, wx) = columns[x];

                        // Two horizontal lerps and one vertical, in
                        // integers. Bilinear on luma only: the eye sees
                        // detail in luminance, and NV12 already stores
                        // chroma at half resolution -- interpolating both
                        // would double the hot loop to improve what
                        // nobody sees.
                        let a = data[l0 + x0] as i32;
                        let b = data[l0 + x1] as i32;
                        let c = data[l1 + x0] as i32;
                        let d = data[l1 + x1] as i32;
                        let top = a * (256 - wx) + b * wx;
                        let bottom = c * (256 - wx) + d * wx;
                        let yy = (top * (256 - wy) + bottom * wy) >> 16;

                        let i_uv = uv_row + (x0 & !1);
                        let u = data[i_uv] as i32;
                        let v = data[i_uv + 1] as i32;

                        let cc = (yy - 16) * 298;
                        let du = u - 128;
                        let dv = v - 128;
                        let r = (cc + 459 * dv + 128) >> 8;
                        let g = (cc - 55 * du - 136 * dv + 128) >> 8;
                        let bl = (cc + 541 * du + 128) >> 8;
                        *px = ((r.clamp(0, 255) as u32) << 16)
                            | ((g.clamp(0, 255) as u32) << 8)
                            | (bl.clamp(0, 255) as u32);
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    /// The conversion has a time budget, and it is testable.
    ///
    /// It runs on the CPU for every drawn frame, so it is the ceiling of
    /// the whole viewer: the single-threaded version cost 28 ms (35 fps)
    /// and held drawing at 27 while the network delivered 60. The user
    /// saw that as "awful quality" -- half of it was the jaggies of
    /// nearest neighbor, half was this loop.
    ///
    /// 12 ms is loose on purpose: we measured 2.8 on a 12-core machine,
    /// and whoever runs the tests may be busy. What matters is not going
    /// back to the 28 range.
    ///
    /// **Release only.** Under `cargo test` without `--release` the same
    /// loop measures 24 ms and the test always fails -- a false alarm that
    /// costs the time of whoever investigates (it cost mine). What it
    /// guards is the optimized code, which is the only one the user runs.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "time budget only holds in --release")]
    fn conversion_fits_the_budget() {
        use std::time::Instant;
        let (sw, sh) = (2560usize, 1440usize);
        let mut data = vec![128u8; sw * sh + sw * sh / 2];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 255) as u8;
        }
        let q = Nv12 { w: sw as u32, h: sh as u32, pitch: sw, data };
        let (dw, dh) = (1451u32, 1232u32);
        let mut dst = vec![0u32; (dw * dh) as usize];
        nv12_to_bgra(&q, &mut dst, dw, dh); // warms cache and pages

        let t = Instant::now();
        let n = 30;
        for _ in 0..n {
            nv12_to_bgra(&q, &mut dst, dw, dh);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("conversion {sw}x{sh} -> {dw}x{dh}: {ms:.2} ms ({:.0} fps)", 1000.0 / ms);
        assert!(ms < 12.0, "conversion at {ms:.1} ms/frame: ceiling of {:.0} fps", 1000.0 / ms);
    }

    use super::*;

    /// Builds a constant-color NV12.
    fn solid(w: u32, h: u32, y: u8, u: u8, v: u8) -> Nv12 {
        let pitch = w as usize + 32; // pitch > width, as NVDEC does
        let mut data = vec![0u8; pitch * (h as usize) * 3 / 2];
        for row in 0..h as usize {
            for x in 0..w as usize {
                data[row * pitch + x] = y;
            }
        }
        let base = pitch * h as usize;
        for row in 0..(h as usize / 2) {
            for x in (0..w as usize).step_by(2) {
                data[base + row * pitch + x] = u;
                data[base + row * pitch + x + 1] = v;
            }
        }
        Nv12 { data, pitch, w, h }
    }

    fn channels(px: u32) -> (u8, u8, u8) {
        (((px >> 16) & 0xff) as u8, ((px >> 8) & 0xff) as u8, (px & 0xff) as u8)
    }

    #[test]
    fn black_and_white_reach_the_extremes() {
        // Limited range: 16 is black, 235 is white. Treating it as full
        // range would give gray instead of black, which is the classic
        // mistake.
        let mut dst = vec![0u32; 4];
        nv12_to_bgra(&solid(2, 2, 16, 128, 128), &mut dst, 2, 2);
        assert_eq!(channels(dst[0]), (0, 0, 0), "16 must become black");

        nv12_to_bgra(&solid(2, 2, 235, 128, 128), &mut dst, 2, 2);
        let (r, g, b) = channels(dst[0]);
        assert!(r > 250 && g > 250 && b > 250, "235 became {r},{g},{b}");
    }

    #[test]
    fn mid_gray_stays_gray() {
        let mut dst = vec![0u32; 4];
        nv12_to_bgra(&solid(2, 2, 126, 128, 128), &mut dst, 2, 2);
        let (r, g, b) = channels(dst[0]);
        assert!(r == g && g == b, "gray came out colored: {r},{g},{b}");
        assert!((100..=140).contains(&r), "mid gray became {r}");
    }

    #[test]
    fn honors_the_pitch_and_reads_no_garbage() {
        // The pitch is larger than the width: reading row by row by `w`
        // instead of by `pitch` would scramble the image progressively --
        // an error that shows up as a slant, not as a failure.
        let w = 8;
        let h = 4;
        let mut q = solid(w, h, 16, 128, 128);
        // Mark the first pixel of each row white.
        for row in 0..h as usize {
            q.data[row * q.pitch] = 235;
        }
        let mut dst = vec![0u32; (w * h) as usize];
        nv12_to_bgra(&q, &mut dst, w, h);
        for row in 0..h as usize {
            let (r, _, _) = channels(dst[row * w as usize]);
            assert!(r > 250, "row {row} lost the mark -- pitch ignored");
            let (r2, _, _) = channels(dst[row * w as usize + 1]);
            assert!(r2 < 5, "row {row} leaked the mark into the neighbor");
        }
    }

    #[test]
    fn shrinking_does_not_overflow_the_destination() {
        // Destination smaller than the source: the loop walks the
        // DESTINATION, so no write can go past the end.
        let q = solid(64, 64, 100, 128, 128);
        let mut dst = vec![0u32; 7 * 5];
        nv12_to_bgra(&q, &mut dst, 7, 5);
        assert!(dst.iter().all(|p| *p != 0), "a pixel was left unwritten");
    }

    #[test]
    fn zero_size_does_nothing() {
        let q = solid(4, 4, 100, 128, 128);
        let mut dst = vec![0u32; 1];
        nv12_to_bgra(&q, &mut dst, 0, 0);
        assert_eq!(dst[0], 0);
    }
}
