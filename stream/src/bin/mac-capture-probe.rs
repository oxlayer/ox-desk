// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standalone runner for the native capture probe.
//!
//! The logic is in `oxdesk_stream::mac_native`, because the host has to be
//! able to run exactly the same code: macOS ties the Screen Recording grant
//! to the binary, so a fresh probe is refused with `-3801` while
//! `oxdesk-host --probe-native` is not. See specs/020-native-mac.md.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("oxdesk-probe-mac-capture only runs on macOS: ScreenCaptureKit only exists there");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(e) = oxdesk_stream::mac_native::probe() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
