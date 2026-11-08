//! NVENC: BGRA frame -> H.264, on the GPU.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! It lives in the library because both hosts are NVIDIA (RTX 3060 on
//! Ubuntu, RTX 3050 on Windows) and `moq-nvenc` loads the library at runtime
//! on both (`libnvidia-encode.so.1` and `nvEncodeAPI64.dll`). So it is one
//! encoding path, not two -- and the quality parameters, which cost a round
//! of "the image looks buggy", stay in a single place.

use cudarc::driver::CudaContext;
use moq_nvenc::sys::nvEncodeAPI::{
    NV_ENC_BUFFER_FORMAT, NV_ENC_CODEC_H264_GUID, NV_ENC_PARAMS_RC_MODE, NV_ENC_PRESET_P4_GUID,
    NV_ENC_TUNING_INFO,
};
use moq_nvenc::{EncodePictureParams, Encoder, EncoderInitParams};

/// Encoding session with its buffers.
///
/// The `Option`s exist because the API consumes the buffers in
/// `encode_picture` and hands them back in `finish` -- this struct is where
/// that back-and-forth stays hidden, instead of showing up in each
/// platform's capture loop.
pub struct NvencEncoder {
    session: Option<moq_nvenc::Session>,
    input: Option<moq_nvenc::Buffer>,
    output: Option<moq_nvenc::Bitstream>,
    w: u32,
    h: u32,
    /// Frame number. Frame zero is forced IDR; the rest follow the GOP.
    n: u64,
    /// The next frame comes out IDR no matter what.
    idr: bool,
    /// What came out of the last encode was IDR.
    last_was_idr: bool,
    /// Requested average bitrate, in bits/s. Kept only so the host can
    /// announce it in the log -- the wrong 8 Mbps target survived for weeks
    /// because the Windows host never printed it, and it only showed up on
    /// the Mac, which did. A number nobody sees is a number nobody checks.
    target: u32,
}

impl NvencEncoder {
    /// Requested average bitrate, in Mbps.
    pub fn target_mbps(&self) -> f64 {
        self.target as f64 / 1_000_000.0
    }

    pub fn new(w: u32, h: u32) -> anyhow::Result<Self> {
        Encoder::load().map_err(|e| anyhow::anyhow!("NVENC: {e:?}"))?;
        let ctx = CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA: {e:?}"))?;
        let enc =
            Encoder::initialize_with_cuda(ctx).map_err(|e| anyhow::anyhow!("session: {e:?}"))?;
        let tuning = NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_LOW_LATENCY;
        let preset = enc
            .get_preset_config(NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P4_GUID, tuning)
            .map_err(|e| anyhow::anyhow!("preset: {e}"))?;
        let mut cfg = preset.presetCfg;
        // Without this, a client joining mid-stream -- the normal case --
        // has no header to start drawing from.
        unsafe { cfg.encodeCodecConfig.h264Config.set_repeatSPSPPS(1) };

        // --- quality -----------------------------------------------------
        //
        // The preset alone picked ~2 Mbps at 1440p, and the user saw the
        // result: "the image looks kind of buggy, not much quality". On a
        // LAN that is bandwidth left on the table -- Parsec uses 20-50.
        // The formula lives in `quality.rs`, not here: it was duplicated in
        // the Mac host, and two copies of the same formula is how the wrong
        // 8 Mbps target survived for weeks.
        let target = crate::quality::target(w, h);
        // ~8 Mbps at 1080p, ~15 at 1440p, ~33 at 4K: scales with the area,
        // because 1440p and 4K do not fit under the same ceiling.
        //
        // It used to be `pixels * 60 / 1000`, which gives 221 kbps at 1440p
        // and **always** fell to the 8 Mbps floor -- at any resolution. The
        // comment promised 15 and 34 and delivered 8 in all three cases; it
        // only came out when the Mac's Retina screen (3360x2100) also
        // announced "8 Mbps target" and the number got absurd enough for me
        // to check the math.
        //
        // 4 bits per pixel per second at 60 fps is what yields the numbers
        // the comment always claimed.

        cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_VBR;
        cfg.rcParams.averageBitRate = target;
        // Ceiling with headroom: a still screen costs almost nothing, and
        // when the whole image changes (scrolling a page) is the time to
        // spend.
        cfg.rcParams.maxBitRate = target * 2;
        // One-frame VBV, sized on the ceiling and not on the average.
        //
        // A large buffer lets the encoder "owe" bits and deliver a late
        // frame, the opposite of what a remote screen wants -- hence one
        // frame, no more. But it was `target / 60`, sized by the average,
        // which made the `maxBitRate` above pure fiction: no frame could
        // exceed 16 KB, so the 2x headroom could never be spent. When
        // scrolling, which is exactly when the person is looking, the
        // encoder was not allowed to spend and raised the QP up to `maxQP`.
        // That is what showed up as blur.
        //
        // The proof was in the log all along: 120 KB keyframes against a
        // 16 KB VBV.
        cfg.rcParams.vbvBufferSize = cfg.rcParams.maxBitRate / 60;
        // Quality floor: `maxQP` bounds the WORST frame. Without it, in a
        // scene that changes entirely the encoder drops quality through the
        // floor to fit the bitrate, and that is exactly where the person is
        // looking.
        cfg.rcParams.set_enableMaxQP(1);
        cfg.rcParams.maxQP.qpIntra = 28;
        cfg.rcParams.maxQP.qpInterP = 30;
        cfg.rcParams.maxQP.qpInterB = 32;

        // --- error recovery ----------------------------------------------
        //
        // There used to be an IDR only at frame zero. Lose one frame and the
        // error NEVER healed -- it smeared the image forever. An IDR every
        // 2s bounds the damage to 2s.
        cfg.gopLength = 120;
        // No B-frames: they improve compression at the cost of delaying
        // output, and here latency is worth more than bitrate.
        cfg.frameIntervalP = 1;
        // Writing to a union field is safe; only reading needs `unsafe`.
        cfg.encodeCodecConfig.h264Config.idrPeriod = 120;

        let mut params = EncoderInitParams::new(NV_ENC_CODEC_H264_GUID, w, h);
        params
            .preset_guid(NV_ENC_PRESET_P4_GUID)
            .tuning_info(tuning)
            .framerate(60, 1)
            .enable_picture_type_decision();
        unsafe { params.encode_config(cfg) };
        let s = enc
            .start_session(NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_ARGB, params)
            .map_err(|e| anyhow::anyhow!("start: {e}"))?;
        let input = s.create_input_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
        let output = s.create_output_bitstream().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(NvencEncoder {
            session: Some(s),
            input: Some(input),
            output: Some(output),
            w,
            h,
            n: 0,
            idr: false,
            last_was_idr: false,
            target,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Encodes a BGRA frame. `stride` is the row pitch of the SOURCE --
    /// both PipeWire and DXGI deliver rows with padding at the end, almost
    /// never equal to the pitch NVENC wants.
    pub fn encode(&mut self, px: &[u8], stride: usize) -> anyhow::Result<Vec<u8>> {
        let (session, mut input, output) = match (
            self.session.take(),
            self.input.take(),
            self.output.take(),
        ) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            // An earlier error left the session without a buffer: there is
            // no way to go on, and putting back what is left keeps the state
            // consistent.
            (a, b, c) => {
                self.session = a;
                self.input = b;
                self.output = c;
                anyhow::bail!("encoder without buffers");
            }
        };

        {
            let mut lock = match input.lock() {
                Ok(l) => l,
                Err(e) => {
                    self.session = Some(session);
                    self.output = Some(output);
                    return Err(anyhow::anyhow!("lock: {e}"));
                }
            };
            let pitch = lock.pitch() as usize;
            let row = (self.w as usize) * 4;
            if pitch == stride {
                let len = (stride * self.h as usize).min(px.len());
                unsafe { lock.write(&px[..len]) };
            } else {
                let packed = repack(px, stride, row, pitch, self.h as usize);
                unsafe { lock.write(&packed) };
            }
        }

        let n = self.n;
        // The first frame, and every requested frame: without an IDR the
        // client that just joined has no way to start drawing, and with a
        // 2s GOP it would stay black that whole time.
        let force = n == 0 || self.idr;
        self.idr = false;
        let r = session.encode_picture(
            input,
            output,
            EncodePictureParams { input_timestamp: n, force_idr: force },
        );
        self.session = Some(session);
        match r.and_then(|s| s.finish()) {
            Ok((data, i2, s2)) => {
                self.input = Some(i2);
                self.output = Some(s2);
                self.n += 1;
                self.last_was_idr = force;
                Ok(data)
            }
            Err(e) => Err(anyhow::anyhow!("encode: {e}")),
        }
    }

    /// Encodes the last frame AGAIN, as an IDR.
    ///
    /// The input buffer still holds the last frame written by `encode`, so
    /// a capture that only delivers on change (Mutter) can answer a
    /// keyframe request on a still screen. Without this the request would
    /// wait for the next screen change, which on an idle desktop is never.
    pub fn reencode_idr(&mut self) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(self.n > 0, "no frame encoded yet");
        let (Some(session), Some(input), Some(output)) =
            (self.session.take(), self.input.take(), self.output.take())
        else {
            anyhow::bail!("encoder without buffers");
        };
        let n = self.n;
        let r = session.encode_picture(
            input,
            output,
            EncodePictureParams { input_timestamp: n, force_idr: true },
        );
        self.session = Some(session);
        match r.and_then(|s| s.finish()) {
            Ok((data, i2, s2)) => {
                self.input = Some(i2);
                self.output = Some(s2);
                self.n += 1;
                self.idr = false;
                self.last_was_idr = true;
                Ok(data)
            }
            Err(e) => Err(anyhow::anyhow!("encode: {e}")),
        }
    }

    /// Asks for the NEXT frame to come out IDR.
    ///
    /// The caller is the network, when a client connects: it has no
    /// context at all, and a P-frame without a reference does not decode.
    /// Without this the window opened black until the GOP's next IDR -- up
    /// to 2s.
    pub fn request_idr(&mut self) {
        self.idr = true;
    }

    /// The last encoded frame was a keyframe.
    ///
    /// This is what goes in the slice header, and the client uses it to know
    /// whether it can start drawing from there.
    pub fn last_was_idr(&self) -> bool {
        self.last_was_idr
    }
}

/// Copies the source rows into the pitch NVENC asked for.
///
/// The destination has `pitch` bytes per row, not `row`: gluing the rows
/// together scrambled the image at every width whose pitch has padding,
/// such as 800 px (3200 bytes). At 1920 and 2560 the pitches coincide, which
/// is why the defect only showed up with the Windows virtual screen.
fn repack(px: &[u8], stride: usize, row: usize, pitch: usize, h: usize) -> Vec<u8> {
    let mut dest = vec![0u8; pitch * h];
    for y in 0..h {
        let start = y * stride;
        if start + row > px.len() {
            break;
        }
        dest[y * pitch..y * pitch + row].copy_from_slice(&px[start..start + row]);
    }
    dest
}

#[cfg(test)]
mod pitch_tests {
    #[test]
    fn each_row_starts_at_the_destination_pitch() {
        // Three roles with different values: source, width and destination.
        let (stride, row, pitch, h) = (6, 4, 8, 2);
        let px = [1, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let d = super::repack(&px, stride, row, pitch, h);
        assert_eq!(d.len(), pitch * h);
        assert_eq!(&d[0..4], &[1, 2, 3, 4]);
        assert_eq!(&d[8..12], &[5, 6, 7, 8]);
    }
}
