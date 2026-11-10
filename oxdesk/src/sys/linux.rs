//! Linux implementation of the contract in `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
pub use super::unix::*;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::sync::{Arc, RwLock};

const UNIT: &str = "oxdesk.service";

/// Resolves through `getent` when the libc resolver gives up.
///
/// `to_socket_addrs` uses getaddrinfo with AF_UNSPEC, and that **fails**
/// for `.local` on this machine: `mdns4_minimal` with `[NOTFOUND=return]`
/// in nsswitch aborts the chain on the AAAA query. Confirmed: `ssh-keyscan
/// windows-pc.local` gives "Temporary failure in name resolution" while
/// `getent hosts` works.
pub fn resolve_fallback(addr: &str, port: u16) -> Option<SocketAddr> {
    let out = Command::new("getent").arg("hosts").arg(addr).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let ip = text.split_whitespace().next()?;
    (ip, port).to_socket_addrs().ok()?.next()
}

pub fn unit_state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Here the journal timestamps and persists; the daemon only writes to
/// stdout.
pub fn log_sink(_line: &str) {}

pub fn log_lines(n: usize) -> Raw {
    let out = Command::new("journalctl")
        .args(["--user", "-u", UNIT, "-n", &n.to_string(), "--no-pager", "-o", "short-iso"])
        .output();

    let Ok(out) = out else {
        return Raw {
            unit: UNIT.into(),
            lines: vec![],
            error: Some("could not run journalctl".into()),
        };
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with("--") {
            continue; // journal header
        }
        // "2026-11-03T18:51:18-0300 host oxdesk[123]: message"
        let Some((time, rest)) = line.split_once(' ') else { continue };
        let msg = match rest.split_once("]: ") {
            Some((_, m)) => m,
            None => rest.split_once(": ").map(|(_, m)| m).unwrap_or(rest),
        };
        lines.push((time.get(11..19).unwrap_or(&time).to_string(), msg.to_string()));
    }
    Raw { unit: UNIT.into(), lines, error: None }
}

pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    crate::border::spawn(cfg);
}

/// Can we open any `/dev/hidraw`?
///
/// The Solaar rule grants through `TAG+="uaccess"`, which becomes an ACL
/// for the logged-in user. A gotcha we paid for: if the nodes were born
/// BEFORE the rule existed, no ACL was applied and only `udevadm trigger`
/// fixes it.
pub fn device_access() -> bool {
    let (_, openable) = hidraw_count();
    openable > 0
}

/// (`hidraw` nodes that exist, how many we can open).
///
/// Both numbers, not just the second, because they tell apart cases that
/// call for opposite actions: zero nodes means there is NO raw HID
/// hardware -- the receiver was unplugged -- and nodes that exist but do
/// not open mean a permission problem. The panel said "sem acesso: rode
/// udevadm" in both cases, and in the first that sends the person to fix
/// a permission that is not broken. It happened: the user took the
/// receiver to the other machine and the panel reported a missing ACL.
pub fn hidraw_count() -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir("/dev") else { return (0, 0) };
    let mut total = 0;
    let mut openable = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if !n.starts_with("hidraw") {
            continue;
        }
        total += 1;
        if std::fs::File::open(e.path()).is_ok() {
            openable += 1;
        }
    }
    (total, openable)
}

/// Known screen clients, in order of preference.
///
/// `xfreerdp3` comes BEFORE `gnome-connections` because it is the only one
/// that exposes sound and microphone on the command line -- and those are
/// exactly the options the UI offers. gnome-connections stays on the list
/// as a fallback: it speaks RDP and VNC and comes preinstalled, it just
/// does not let you choose anything.
pub fn screen_clients(kind: &str) -> Vec<super::ClientSpec> {
    use super::ClientSpec;
    match kind {
        // `/dynamic-resolution` makes the remote host follow the window;
        // without it the session is born at the server's resolution and
        // stays letterboxed. `+clipboard` is what makes copy and paste
        // between the two machines work inside the session.
        "rdp" => vec![
            ClientSpec {
                prog: "xfreerdp3",
                args: &["/v:{host}:{port}", "{u_flag}", "+clipboard", "/dynamic-resolution"],
                audio: Some(&["/sound"]),
                mic: Some(&["/microphone"]),
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "xfreerdp",
                args: &["/v:{host}:{port}", "{u_flag}", "+clipboard", "/dynamic-resolution"],
                audio: Some(&["/sound"]),
                mic: Some(&["/microphone"]),
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "gnome-connections",
                args: &["rdp://{user_at}{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "remmina",
                args: &["-c", "rdp://{user_at}{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
        ],
        "vnc" => vec![
            // vncviewer comes first because of `-ViewOnly`: it is the only
            // way, on this side, to come in without keyboard and mouse.
            ClientSpec {
                prog: "vncviewer",
                args: &["{host}:{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-ViewOnly"]),
                file: None,
            },
            ClientSpec {
                prog: "gnome-connections",
                args: &["vnc://{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
            ClientSpec {
                prog: "remmina",
                args: &["-c", "vnc://{host}:{port}"],
                audio: None,
                mic: None,
                view_only: None,
                file: None,
            },
        ],
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

/// Where to look for clients outside PATH.
///
/// `~/.local/bin` is included because that is where OUR binaries live,
/// and the daemon runs with a minimal PATH -- under systemd or launchd it
/// does not inherit the shell's PATH, and looking only in PATH would make
/// `oxdesk-view` look uninstalled when it is installed.

pub fn screen_client_dirs() -> Vec<std::path::PathBuf> {
    vec![home_dir().join(".local").join("bin")]
}
