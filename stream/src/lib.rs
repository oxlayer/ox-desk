// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Our own remote screen. Spec: ../specs/010-remote-desktop.md
//!
//! The boundaries matter more than the first implementation: the first
//! encoder that works will not be the last, and swapping NVENC for
//! VideoToolbox or x264 must not mean rewriting the pipeline.
//!
//! ## How the pieces split
//!
//! What is the SAME across platforms lives in `serve` (video loop, slices,
//! exit on idle) and `nvenc` (encoding and quality). What genuinely
//! differs lives in pairs: `capture`/`capture_win` for the frame source,
//! and input injection (`host_linux` talks to Mutter over D-Bus,
//! `input_win` calls `SendInput`).
//!
//! Platform modules are `#![cfg(...)]` inside, so on the wrong OS they
//! compile EMPTY instead of failing -- the Mac compiles this whole
//! library just to build the viewer.

pub mod bind;
pub mod capture;
pub mod capture_input;
pub mod capture_win;
pub mod cursor;
pub mod cursor_win;
pub mod decode;
pub mod decode_nv;
pub mod encode;
// Empty off Windows: the file carries its own `#![cfg(windows)]`.
pub mod grab_win;
pub mod host_linux;
pub mod host_mac;
pub mod host_win;
pub mod identity;
pub mod input;
pub mod input_mac;
// Empty off macOS: the file carries its own `#![cfg]`.
pub mod mac_native;
pub mod input_win;
pub mod net;
pub mod nvenc;
pub mod quality;
pub mod serve;
// Empty on other targets: the file carries its own `#![cfg(target_os = "macos")]`,
// so the crate still cross-builds whole.
pub mod vt;
pub mod telemetry;
