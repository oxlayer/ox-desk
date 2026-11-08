//! Standalone runner for the native capture probe.
//!
//! The logic is in `oxdesk_stream::mac_native`, because the host has to be
//! able to run exactly the same code: macOS ties the Screen Recording grant
//! to the binary, so a fresh probe is refused with `-3801` while
//! `oxdesk-host --probe-nativo` is not. See specs/020-native-mac.md.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("oxdesk-probe-mac-capture so roda no macOS: ScreenCaptureKit so existe la");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() {
    if let Err(e) = oxdesk_stream::mac_native::probe() {
        eprintln!("ERRO: {e}");
        std::process::exit(1);
    }
}
