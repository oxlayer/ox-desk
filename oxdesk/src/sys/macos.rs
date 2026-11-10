// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! macOS implementation of the contract in `mod.rs`.
//!
//! What is the same as Linux comes from `unix.rs` (local time, config
//! directory, home, child builder). What differs lives here.

use crate::config::Config;
use crate::logs::Raw;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub use super::unix::*;

/// macOS's mDNSResponder resolves `.local` natively, so the hole that
/// `getent` plugs on Linux does not exist.
pub fn resolve_fallback(_addr: &str, _port: u16) -> Option<SocketAddr> {
    None
}

/// State of a config `units` entry, which here is neither a unit nor a
/// service.
///
/// Same stance as Windows: try the OS manager (launchd) and, if there is
/// nothing with that label, fall back to "a process with that name
/// exists". Both cases show up -- `com.apple.screensharing` is a launchd
/// service, `oxdesk` is a loose process in the user's session.
pub fn unit_state(unit: &str) -> String {
    // ABSOLUTE PATH, not the bare name. Under launchd PATH is minimal, and
    // `pgrep` did not resolve -- the panel said `oxdesk=inactive` about the
    // very process answering the request.
    let uid = unsafe { libc_getuid() };
    // The user domain first, then the system one: Apple services
    // (com.apple.screensharing) live in `system/`, and the user's
    // `launchctl list` does not see them.
    for target in [format!("gui/{uid}/{unit}"), format!("system/{unit}")] {
        let ok = super::unix::child("/bin/launchctl")
            .args(["print", &target])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return "active".into();
        }
    }
    let running = super::unix::child("/usr/bin/pgrep")
        .args(["-x", unit])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if running {
        "active".into()
    } else {
        "inactive".into()
    }
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LINES: usize = 500;

/// `~/Library/Logs/oxdesk/oxdesk.log` -- the place Console.app opens.
pub fn log_path() -> PathBuf {
    home_dir().join("Library").join("Logs").join("oxdesk").join("oxdesk.log")
}

/// Without a journal, the daemon timestamps and persists its own lines.
///
/// Rotates by size, like Windows: a resident daemon that writes border
/// and battery events fills a file, and a log that grows without bound
/// stops being a log and becomes a disk problem.
pub fn log_sink(line: &str) {
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let kept: Vec<&str> = text.lines().rev().take(KEEP_LINES).collect();
            let body: String = kept.into_iter().rev().map(|l| format!("{l}\n")).collect();
            let _ = std::fs::write(&p, body);
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", hhmmss(), line);
    }
}

pub fn log_lines(n: usize) -> Raw {
    let p = log_path();
    let label = p.to_string_lossy().to_string();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Raw { unit: label, lines: vec![], error: Some("there is no log file yet".into()) };
    };
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(n);
    let lines = all[start..]
        .iter()
        .filter_map(|l| l.split_once(' '))
        .map(|(t, m)| (t.to_string(), m.to_string()))
        .collect();
    Raw { unit: label, lines, error: None }
}

/// Can we talk to an HID++ receiver?
///
/// On this machine the answer today is **no**, for a hardware reason: the
/// MacBook has no Logitech USB receiver -- mouse and keyboard reach it
/// over Bluetooth, and this project's HID++ talks to the receiver.
/// `hidapi` is still queried instead of returning a fixed `false`,
/// because plugging in a receiver would solve it, and on that day the
/// code should not need to change.
pub fn device_access() -> bool {
    hidpp::hidapi::HidApi::new()
        .ok()
        .map(|api| hidpp::Receiver::open(&api).is_ok())
        .unwrap_or(false)
}

/// There is no border watch on macOS -- and it is not just missing code.
///
/// Reading the cursor here is easy (`CGEventGetLocation`) and so is the
/// idle time (`CGEventSourceSecondsSinceLastEventType`). What is missing
/// is the other side: without a receiver there is no way to **push** the
/// device to another channel over HID++. A border that detects and does
/// not switch would be worse than none -- it would promise something that
/// does not happen, which is the bug pattern this project has already
/// paid for twice (the `backfill_gate` flag nobody read, the `langfuse`
/// that swallowed errors).
///
/// To LEAVE the Mac, use the Easy-Switch button. For the Mac to RECEIVE,
/// another machine just has to push -- and for that its `focus::plan`
/// requires an `agent` here, which only makes sense once there is a
/// receiver.
pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    let enabled = cfg.read().map(|g| g.input.border.enabled).unwrap_or(false);
    if enabled {
        gripe!(
            "border: enabled in the config, but there is no border watch on macOS -- \
             without a Logitech receiver there is no way to push the device over HID++. \
             Leave through the Easy-Switch button. See sys/macos.rs."

        );
    }
}

/// Known screen clients, in order of preference.
///
/// VNC is native: `open vnc://` opens Apple's Screen Sharing, which comes
/// preinstalled -- but with no command-line options at all. Nothing comes
/// for RDP; the way is `Windows App` (formerly Microsoft Remote Desktop),
/// via `brew install --cask windows-app`.
pub fn screen_clients(kind: &str) -> Vec<super::ClientSpec> {
    use super::ClientSpec;
    match kind {
        "vnc" => vec![ClientSpec {
            prog: "open",
            args: &["vnc://{host}:{port}"],
            audio: None,
            mic: None,
            view_only: None,
            file: None,
        }],
        // An `.rdp` file, not the `rdp://` scheme: `Windows App` REFUSES
        // the URL ("Unable to interpret ... as a path or URL") and opens
        // the file without complaint. Verified on this machine.
        //
        // As a bonus, the file carries sound and microphone -- so the GUI
        // options apply here, which the command line would not give.
        // `-g` opens without stealing focus from whoever is using the
        // machine.
        "rdp" => vec![ClientSpec {
            prog: "open",
            args: &["-g", "{file}"],
            audio: Some(&[]),
            mic: Some(&[]),
            view_only: None,
            file: Some("full address:s:{host}:{port}\\n{user_line}screen mode id:i:2\\nredirectclipboard:i:1\\naudiomode:i:{audiomode}\\naudiocapturemode:i:{audiocapture}\\n"),
        }],
        // OURS. The same binary on every platform, and the only one that
        // speaks the `oxdesk-stream` protocol -- H.264 over QUIC with input
        // coming back. See specs/010.
        "oxdesk-stream" => vec![ClientSpec {
            prog: "oxdesk-view",
            args: &["{host}:{port}"],
            // No audio or view-only mode yet; when there is, it changes
            // here and the UI lights up on its own.
            audio: None,
            mic: None,
            view_only: None,
            file: None,
        }],
        _ => vec![],
    }
}

pub fn screen_client_dirs() -> Vec<PathBuf> {
    vec![
        // OUR binaries. The daemon runs under launchd with a minimal PATH
        // and does not inherit the shell's; without this
        // `oxdesk-view` looks uninstalled when it is installed.

        home_dir().join(".local").join("bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/Applications"),
    ]
}
