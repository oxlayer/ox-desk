//! Hardware H.264 encoding on macOS, straight against VideoToolbox.
//!
//! Spec: ../../specs/020-native-mac.md
//!
//! This is what `h264_videotoolbox` wraps. Calling it directly removes
//! Homebrew's FFmpeg from the Mac -- the dependency that produced a package
//! that built and could not be installed, a "versioned prefix" that is a
//! hand-made symlink, and the GPL-3 library inside what we distribute.
//!
//! The FFI is written by hand because the surface is small and C: four
//! VideoToolbox calls, five CoreMedia, five CoreVideo. The capture half
//! (ScreenCaptureKit) is Objective-C and does use bindings, which is a
//! different trade -- those are compile-time, permissive and need nothing
//! installed.
#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

/// `CMTime`, and the field order matters more than it looks: value,
/// timescale, flags, epoch. Getting it wrong does not fail to compile, it
/// produces timestamps the encoder silently misreads.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CMTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

impl CMTime {
    /// A valid time of `value/timescale` seconds.
    pub fn new(value: i64, timescale: i32) -> Self {
        CMTime { value, timescale, flags: 1, epoch: 0 }
    }
    pub fn invalid() -> Self {
        CMTime { value: i64::MAX, timescale: 60, flags: 0, epoch: 0 }
    }
}

type OSStatus = i32;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub fn CVPixelBufferCreate(
        allocator: *const c_void,
        width: usize,
        height: usize,
        pixel_format: u32,
        attributes: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    pub fn CVPixelBufferLockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    pub fn CVPixelBufferUnlockBaseAddress(buf: *mut c_void, flags: u64) -> i32;
    pub fn CVPixelBufferGetBaseAddress(buf: *mut c_void) -> *mut c_void;
    pub fn CVPixelBufferGetBytesPerRow(buf: *mut c_void) -> usize;
    pub fn CVPixelBufferGetWidth(buf: *mut c_void) -> usize;
    pub fn CVPixelBufferGetHeight(buf: *mut c_void) -> usize;
    pub fn CVPixelBufferRelease(buf: *mut c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub fn CMSampleBufferGetImageBuffer(sbuf: *mut c_void) -> *mut c_void;
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

/// `'BGRA'` as an OSType -- the four-character codes are literally the bytes.
pub const PIXEL_BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// `'avc1'` -- H.264.
const CODEC_H264: u32 = u32::from_be_bytes(*b"avc1");

/// One encoded access unit, in AVCC form: each NAL unit prefixed by its
/// 4-byte big-endian length.
#[derive(Default, Clone)]
pub struct Quadro {
    pub bytes: Vec<u8>,
    /// SPS and PPS, which VideoToolbox keeps in the format description and
    /// NOT in the frame data -- the same split NVENC has. A decoder needs
    /// them before the first frame.
    pub parametros: Vec<Vec<u8>>,
}

#[derive(Default)]
struct Caixa {
    pronto: Option<Quadro>,
    status: OSStatus,
    chamadas: usize,
}

/// A compression session. `encode` is synchronous on purpose: it hands the
/// frame over and waits for the callback, which keeps the first version
/// easy to reason about. Pipelining comes later, with a measurement to
/// justify it.
pub struct Encoder {
    sessao: *mut c_void,
    caixa: Arc<Mutex<Caixa>>,
}

extern "C" fn pronto(
    refcon: *mut c_void,
    _source_frame: *mut c_void,
    status: OSStatus,
    _flags: u32,
    sample: *mut c_void,
) {
    // The refcon is an `Arc` we leaked into the session; it stays alive as
    // long as the session does, and `Drop` below takes it back.
    let caixa = unsafe { &*(refcon as *const Mutex<Caixa>) };
    let mut c = match caixa.lock() {
        Ok(c) => c,
        Err(e) => e.into_inner(),
    };
    c.chamadas += 1;
    c.status = status;
    if status != 0 || sample.is_null() {
        return;
    }
    unsafe {
        let bbuf = CMSampleBufferGetDataBuffer(sample);
        if bbuf.is_null() {
            return;
        }
        let len = CMBlockBufferGetDataLength(bbuf);
        let mut bytes = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(bbuf, 0, len, bytes.as_mut_ptr() as *mut c_void) != 0 {
            return;
        }
        let mut parametros = Vec::new();
        let desc = CMSampleBufferGetFormatDescription(sample);
        if !desc.is_null() {
            for i in 0..2usize {
                let mut p: *const u8 = std::ptr::null();
                let mut n: usize = 0;
                let mut count: usize = 0;
                let mut header: i32 = 0;
                if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                    desc, i, &mut p, &mut n, &mut count, &mut header,
                ) == 0
                    && !p.is_null()
                {
                    parametros.push(std::slice::from_raw_parts(p, n).to_vec());
                }
            }
        }
        c.pronto = Some(Quadro { bytes, parametros });
    }
}

impl Encoder {
    /// A null encoder specification means "VideoToolbox chooses", which on
    /// Apple Silicon is the hardware encoder.
    pub fn new(width: i32, height: i32) -> Result<Encoder, String> {
        let caixa = Arc::new(Mutex::new(Caixa::default()));
        let refcon = Arc::as_ptr(&caixa) as *mut c_void;
        let mut sessao: *mut c_void = std::ptr::null_mut();
        let st = unsafe {
            VTCompressionSessionCreate(
                std::ptr::null(),
                width,
                height,
                CODEC_H264,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                Some(pronto),
                refcon,
                &mut sessao,
            )
        };
        if st != 0 || sessao.is_null() {
            return Err(format!("VTCompressionSessionCreate: OSStatus {st}"));
        }
        Ok(Encoder { sessao, caixa })
    }

    /// Encodes one `CVPixelBuffer`. The buffer is borrowed, not consumed.
    pub fn encode(&mut self, pixels: *mut c_void, pts: CMTime, dur: CMTime) -> Result<Quadro, String> {
        {
            let mut c = self.caixa.lock().map_err(|e| e.to_string())?;
            c.pronto = None;
            c.status = 0;
        }
        let st = unsafe {
            VTCompressionSessionEncodeFrame(
                self.sessao,
                pixels,
                pts,
                dur,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if st != 0 {
            return Err(format!("VTCompressionSessionEncodeFrame: OSStatus {st}"));
        }
        // Blocks until every pending callback has run, which is what makes
        // reading `caixa` right after this sound.
        unsafe { VTCompressionSessionCompleteFrames(self.sessao, CMTime::invalid()) };
        let c = self.caixa.lock().map_err(|e| e.to_string())?;
        if c.status != 0 {
            return Err(format!("callback: OSStatus {}", c.status));
        }
        c.pronto.clone().ok_or_else(|| "o encoder nao devolveu quadro".to_string())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // Invalidate FIRST: it waits for callbacks in flight, and only then
        // is it safe to let the refcon's `Arc` go.
        unsafe { VTCompressionSessionInvalidate(self.sessao) };
    }
}

#[cfg(test)]
mod teste {
    use super::*;

    /// The four-character codes are bytes, and a wrong endianness here
    /// would ask VideoToolbox for a codec nobody has.
    #[test]
    fn os_codigos_de_quatro_letras_sao_os_bytes() {
        assert_eq!(PIXEL_BGRA, 0x42475241);
        assert_eq!(CODEC_H264, 0x61766331);
    }

    #[test]
    fn cmtime_valido_tem_a_flag() {
        let t = CMTime::new(3, 60);
        assert_eq!((t.value, t.timescale, t.flags), (3, 60, 1));
        assert_eq!(CMTime::invalid().flags, 0, "sem a flag de valido");
    }
}
