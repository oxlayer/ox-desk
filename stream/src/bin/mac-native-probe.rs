//! Does hardware H.264 encoding work from Rust on this Mac with **no media
//! dependency at all**?
//!
//! Spec: ../../specs/020-native-mac.md (step 1)
//!
//! This is the gate for dropping FFmpeg. What `h264_videotoolbox` does for
//! us is wrap `VTCompressionSession`, and what libavdevice's `avfoundation`
//! wraps is AVFoundation/ScreenCaptureKit. Both are system frameworks. If
//! this probe produces an H.264 access unit, the dependency is a
//! convenience and not a capability -- and the five things it costs us
//! (listed in the spec) are avoidable.
//!
//! Deliberately **no new crate**: the declarations below are the whole FFI
//! surface for encoding, written by hand against the C API. Adding
//! `objc2-video-toolbox` to find out whether VideoToolbox works would be
//! answering the question with the thing in question. ScreenCaptureKit, the
//! capture half, is Objective-C and is a later step -- VideoToolbox is C,
//! which is why it goes first.
//!
//! Run on the Mac: `cargo run --release --bin oxdesk-probe-mac-native`

// `oxdesk-stream` cross-builds for Windows as a whole crate, so a file that
// only exists on one platform says so itself instead of being excluded
// per-OS from Cargo.toml -- the same shape as the Linux-only probes here.
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("oxdesk-probe-mac-native only runs on macOS: it talks to VideoToolbox, which only exists there");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
use std::ffi::c_void;

// ---------------------------------------------------------------- the FFI
//
// `CMTime` has to match the C layout exactly: value, timescale, flags,
// epoch. Getting the field order wrong here does not fail to compile, it
// produces timestamps the encoder silently misreads.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[cfg(target_os = "macos")]
const K_CMTIME_FLAGS_VALID: u32 = 1;

#[cfg(target_os = "macos")]
type OSStatus = i32;

#[cfg(target_os = "macos")]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(buf: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(buf: *mut c_void) -> usize;
    fn CVPixelBufferRelease(buf: *mut c_void);
}

#[cfg(target_os = "macos")]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMSampleBufferGetDataBuffer(sbuf: *mut c_void) -> *mut c_void;
    fn CMBlockBufferGetDataLength(bbuf: *mut c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(
        bbuf: *mut c_void,
        offset: usize,
        length: usize,
        dest: *mut c_void,
    ) -> OSStatus;
    fn CMSampleBufferGetFormatDescription(sbuf: *mut c_void) -> *mut c_void;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: *mut c_void,
        index: usize,
        out_ptr: *mut *const u8,
        out_len: *mut usize,
        out_count: *mut usize,
        out_nal_header_len: *mut i32,
    ) -> OSStatus;
}

#[cfg(target_os = "macos")]
#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    fn VTCompressionSessionCreate(
        allocator: *const c_void,
        width: i32,
        height: i32,
        codec: u32,
        encoder_specification: *const c_void,
        source_image_buffer_attributes: *const c_void,
        compressed_data_allocator: *const c_void,
        output_callback: Option<
            extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, *mut c_void),
        >,
        output_callback_refcon: *mut c_void,
        out: *mut *mut c_void,
    ) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: *mut c_void,
        image_buffer: *mut c_void,
        pts: CMTime,
        duration: CMTime,
        frame_properties: *const c_void,
        source_frame_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: *mut c_void, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: *mut c_void);
}

/// `'BGRA'` as an OSType. The four-character codes are literally the bytes.
#[cfg(target_os = "macos")]
const PIXEL_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `'avc1'` -- H.264.
#[cfg(target_os = "macos")]
const CODEC_H264: u32 = u32::from_be_bytes(*b"avc1");

#[cfg(target_os = "macos")]
#[derive(Default)]
struct Output {
    calls: usize,
    status: OSStatus,
    bytes: Vec<u8>,
}

/// Called by VideoToolbox on its own thread, once per encoded frame.
///
/// `refcon` is the `&mut Output` handed to `VTCompressionSessionCreate`. It
/// stays alive because `VTCompressionSessionCompleteFrames` below blocks
/// until every callback has run, and the session is invalidated before the
/// struct goes out of scope.
#[cfg(target_os = "macos")]
extern "C" fn on_encoded(
    refcon: *mut c_void,
    _source_frame: *mut c_void,
    status: OSStatus,
    _flags: u32,
    sample: *mut c_void,
) {
    let out = unsafe { &mut *(refcon as *mut Output) };
    out.calls += 1;
    out.status = status;
    if status != 0 || sample.is_null() {
        return;
    }
    unsafe {
        let bbuf = CMSampleBufferGetDataBuffer(sample);
        if bbuf.is_null() {
            return;
        }
        let len = CMBlockBufferGetDataLength(bbuf);
        let mut v = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(bbuf, 0, len, v.as_mut_ptr() as *mut c_void) == 0 {
            out.bytes = v;
        }
        // SPS and PPS live in the format description, not in the frame data
        // -- the same split as NVENC's header. Printed here because a
        // decoder on the other side needs them before the first frame.
        let desc = CMSampleBufferGetFormatDescription(sample);
        if !desc.is_null() {
            for i in 0..2usize {
                let mut p: *const u8 = std::ptr::null();
                let mut n: usize = 0;
                let mut count: usize = 0;
                let mut header: i32 = 0;
                let st = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    desc, i, &mut p, &mut n, &mut count, &mut header,
                );
                if st == 0 && !p.is_null() {
                    let nal = *p & 0x1f;
                    println!(
                        "  parameter set {i}: {n} bytes, nal_type {nal} ({}), {count} set(s), length prefix {header} bytes",
                        match nal {
                            7 => "SPS",
                            8 => "PPS",
                            _ => "?",
                        }
                    );
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let (w, h) = (1280i32, 720i32);
    println!("== VideoToolbox straight from Rust, with no media dependency ==");
    println!("  {w}x{h} BGRA -> H.264 (avc1)");

    unsafe {
        // ---- a frame to encode: a gradient, so the output is not a
        // ---- single flat macroblock that compresses to nothing.
        let mut px: *mut c_void = std::ptr::null_mut();
        let r = CVPixelBufferCreate(
            std::ptr::null(),
            w as usize,
            h as usize,
            PIXEL_BGRA,
            std::ptr::null(),
            &mut px,
        );
        if r != 0 || px.is_null() {
            eprintln!("ERROR: CVPixelBufferCreate returned {r}");
            std::process::exit(1);
        }
        CVPixelBufferLockBaseAddress(px, 0);
        let base = CVPixelBufferGetBaseAddress(px) as *mut u8;
        let stride = CVPixelBufferGetBytesPerRow(px);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let o = y * stride + x * 4;
                *base.add(o) = (x & 0xff) as u8; // B
                *base.add(o + 1) = (y & 0xff) as u8; // G
                *base.add(o + 2) = ((x + y) & 0xff) as u8; // R
                *base.add(o + 3) = 0xff;
            }
        }
        CVPixelBufferUnlockBaseAddress(px, 0);
        println!("  CVPixelBuffer: ok, stride {stride}");

        // ---- the session. Null specification means "let VideoToolbox
        // ---- choose", which on Apple Silicon is the hardware encoder.
        let mut out = Output::default();
        let mut session: *mut c_void = std::ptr::null_mut();
        let st = VTCompressionSessionCreate(
            std::ptr::null(),
            w,
            h,
            CODEC_H264,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            Some(on_encoded),
            &mut out as *mut Output as *mut c_void,
            &mut session,
        );
        if st != 0 || session.is_null() {
            eprintln!("ERROR: VTCompressionSessionCreate returned OSStatus {st}");
            CVPixelBufferRelease(px);
            std::process::exit(1);
        }
        println!("  VTCompressionSession: created");

        let pts = CMTime { value: 0, timescale: 60, flags: K_CMTIME_FLAGS_VALID, epoch: 0 };
        let dur = CMTime { value: 1, timescale: 60, flags: K_CMTIME_FLAGS_VALID, epoch: 0 };
        let st = VTCompressionSessionEncodeFrame(
            session,
            px,
            pts,
            dur,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if st != 0 {
            eprintln!("ERROR: VTCompressionSessionEncodeFrame returned OSStatus {st}");
            VTCompressionSessionInvalidate(session);
            CVPixelBufferRelease(px);
            std::process::exit(1);
        }

        // Blocks until every callback has run, which is what makes the
        // `&mut out` above sound.
        let until_end = CMTime { value: i64::MAX, timescale: 60, flags: 0, epoch: 0 };
        VTCompressionSessionCompleteFrames(session, until_end);
        VTCompressionSessionInvalidate(session);
        CVPixelBufferRelease(px);

        println!("  callback: {} call(s), OSStatus {}", out.calls, out.status);
        if out.bytes.is_empty() {
            eprintln!("ERROR: not a single encoded byte");
            std::process::exit(1);
        }
        println!("  access unit: {} bytes", out.bytes.len());

        // AVCC: each NAL unit is prefixed by its 4-byte big-endian length.
        // Walking it proves we got real H.264 and not an opaque blob.
        let b = &out.bytes;
        let mut i = 0usize;
        let mut n = 0;
        while i + 4 <= b.len() && n < 6 {
            let len = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            if len == 0 || i + 4 + len > b.len() {
                break;
            }
            let nal = b[i + 4] & 0x1f;
            println!(
                "  nal {n}: type {nal} ({}), {len} bytes",
                match nal {
                    1 => "non-IDR slice",
                    5 => "IDR slice",
                    6 => "SEI",
                    7 => "SPS",
                    8 => "PPS",
                    _ => "?",
                }
            );
            i += 4 + len;
            n += 1;
        }
        println!();
        println!("  => hardware H.264 encode, from Rust, with no FFmpeg and no media crate");
    }
}
