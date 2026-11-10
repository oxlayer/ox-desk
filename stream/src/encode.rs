// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Raw frame -> H.264.
//!
//! NVENC **directly**, through `libnvidia-encode`, and not through libavcodec:
//! linuxbrew's ffmpeg came without the nvenc encoder (`ffmpeg -encoders` lists
//! only `libx264` and `h264_v4l2m2m`), and libavcodec would be one more layer
//! in the hot path anyway. The library is already on the machine -- it ships
//! with the driver (`libnvidia-encode.so.1`, 580.178.04).
//!
//! Both hosts are NVIDIA (RTX 3060 and RTX 3050), so this is one encoding
//! path, not three.

/// An encoded packet, ready to be framed and sent.
pub struct Packet {
    pub data: Vec<u8>,
    /// Keyframe: a client joining mid-stream needs one before it can start
    /// drawing.
    pub keyframe: bool,
    pub captured_at_ns: u64,
}

pub trait Encoder: Send {
    fn encode(&mut self, frame: &crate::capture::Frame) -> anyhow::Result<Vec<Packet>>;
    /// Keyframe request -- the client asks for one when it loses context.
    fn request_keyframe(&mut self);
}
