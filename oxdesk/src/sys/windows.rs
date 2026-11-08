//! Windows implementation of the contract in `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The Windows resolver has spoken mDNS since 10, so the hole `getent`
/// plugs on Linux does not exist: `to_socket_addrs` is enough.
pub fn resolve_fallback(_addr: &str, _port: u16) -> Option<SocketAddr> {
    None
}

/// State of a config `units` entry, which on Windows is no unit at all.
///
/// The field name stays (the config is the same on both machines), but
/// the reading is different: first we try a real service, and only if it
/// does not exist do we fall back to "a process with that name exists".
/// Both cases show up in practice -- `sshd` is a service, `oxdesk` is a
/// loose process in the user's session.
pub fn unit_state(unit: &str) -> String {
    match service_state(unit) {
        Some(s) => s,
        None => {
            if process_running(unit) {
                "active".into()
            } else {
                "inactive".into()
            }
        }
    }
}

/// `None` = no service with that name exists
/// (ERROR_SERVICE_DOES_NOT_EXIST).
fn service_state(name: &str) -> Option<String> {
    let out = child("sc").args(["query", name]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return None;
    }
    // The line's label is localized ("ESTADO" on a pt-BR Windows), but
    // the state name comes from the SCM as an English constant. Even so
    // we also match on the numeric code, which does not depend on locale.
    let up = text.to_ascii_uppercase();
    let by_name = [
        ("RUNNING", "active"),
        ("STOPPED", "inactive"),
        ("START_PENDING", "activating"),
        ("STOP_PENDING", "deactivating"),
        ("PAUSED", "inactive"),
    ]
    .iter()
    .find(|(tok, _)| up.contains(tok))
    .map(|(_, s)| s.to_string());
    if by_name.is_some() {
        return by_name;
    }
    for (code, state) in [(": 4", "active"), (": 1", "inactive"), (": 2", "activating")] {
        if up.contains(code) {
            return Some(state.into());
        }
    }
    Some("unknown".into())
}

fn process_running(name: &str) -> bool {
    let exe = if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    };
    let out = child("tasklist")
        .args(["/FO", "CSV", "/NH", "/FI", &format!("IMAGENAME eq {exe}")])
        .output();
    let Ok(out) = out else { return false };
    // When nothing matches, tasklist writes "INFO: nenhuma tarefa..." --
    // a localized message. Looking for the exe name avoids depending on
    // it.
    String::from_utf8_lossy(&out.stdout)
        .to_ascii_lowercase()
        .contains(&exe.to_ascii_lowercase())
}

#[repr(C)]
#[derive(Default)]
struct SysTime {
    year: u16,
    month: u16,
    day_of_week: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLocalTime(st: *mut SysTime);
}

/// Local time straight from Win32 -- no `date +%z` nor timezone
/// arithmetic, because here the OS already hands over the local wall
/// clock.
pub fn hhmmss() -> String {
    let mut st = SysTime::default();
    unsafe { GetLocalTime(&mut st) };
    format!("{:02}:{:02}:{:02}", st.hour, st.minute, st.second)
}

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP_LINES: usize = 500;

pub fn log_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oxdesk").join("oxdesk.log")
}

/// Without a journal, the daemon timestamps and persists its own lines.
///
/// Rotates by size: a resident daemon that writes border and battery
/// events fills a file, and a log that grows without bound stops being a
/// log and becomes a disk problem.
pub fn log_sink(line: &str) {
    let p = log_path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let kept: Vec<&str> = text.lines().rev().take(KEEP_LINES).collect();
            let body: String =
                kept.into_iter().rev().map(|l| format!("{l}\n")).collect();
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
        return Raw {
            unit: label,
            lines: vec![],
            error: Some("ainda nao ha arquivo de log".into()),
        };
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

pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    crate::border_win::spawn(cfg);
}

/// Can we talk to the receiver?
///
/// On Windows there is no `/dev/hidraw` nor `uaccess` ACL: access to the
/// vendor collection (usage page 0xFF00) is allowed by default, and what
/// is left is opening it and seeing. Checking the right thing matters --
/// the Linux version of this check looks for a file in `/dev`, and here it
/// would answer "no access" forever, blocking every switch plan.
pub fn device_access() -> bool {
    hidpp::hidapi::HidApi::new()
        .ok()
        .map(|api| hidpp::Receiver::open(&api).is_ok())
        .unwrap_or(false)
}

/// Where the config lives: `%APPDATA%\oxdesk`.
///
/// `HOME` does not exist on Windows. The Linux version falls back to
/// `/tmp` when it cannot find it -- here that would mean writing the
/// config to a meaningless path and losing it on the next boot.
pub fn config_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oxdesk")
}

/// Where the logs of processes we launch live.
///
/// `%LOCALAPPDATA%\oxdesk`, which is where `oxdesk-host` already
/// writes its own -- a single directory to investigate.
pub fn state_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("oxdesk");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Starting point of the file browser.
pub fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| "C:\\".into())
}

/// Known screen clients, in order of preference.
///
/// RDP comes with Windows (`mstsc`), but it does not take sound or
/// microphone on the command line -- that requires generating an `.rdp`
/// file, which is not worth the complexity for now. No VNC comes at all:
/// it is what is missing to reach the Mac from here.
pub fn screen_clients(kind: &str) -> Vec<super::ClientSpec> {
    use super::ClientSpec;
    match kind {
        // Through the FILE and not `/v:`: `mstsc` accepts both, but only
        // the file carries sound and microphone -- its command line does
        // not have those options.
        "rdp" => vec![ClientSpec {
            prog: "mstsc.exe",
            args: &["{file}"],
            audio: Some(&[]),
            mic: Some(&[]),
            view_only: None,
            file: Some("full address:s:{host}:{port}\\n{user_line}screen mode id:i:2\\nredirectclipboard:i:1\\naudiomode:i:{audiomode}\\naudiocapturemode:i:{audiocapture}\\n"),
        }],
        "vnc" => vec![
            ClientSpec {
                prog: "vncviewer.exe",
                args: &["{host}:{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-ViewOnly"]),
                file: None,
            },
            ClientSpec {
                prog: "tvnviewer.exe",
                args: &["{host}::{port}"],
                audio: None,
                mic: None,
                view_only: Some(&["-viewonly"]),
                file: None,
            },
        ],
        // OURS. The same binary on every platform, and the only one that
        // speaks the `oxdesk-stream` protocol -- H.264 over QUIC with input
        // coming back. See specs/010.
        "oxdesk-stream" => vec![ClientSpec {
            prog: "oxdesk-view.exe",
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

/// Installers do not put a VNC viewer on PATH; these are the places where
/// RealVNC and TightVNC land by default.
pub fn screen_client_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    // Where OUR binaries live, next to the daemon.
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        v.push(PathBuf::from(local).join("oxdesk"));
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(key) {
            v.push(PathBuf::from(&base).join("RealVNC").join("VNC Viewer"));
            v.push(PathBuf::from(&base).join("TightVNC"));
            v.push(PathBuf::from(&base).join("uvnc bvba").join("UltraVNC"));
        }
    }
    v
}

/// Creates a process without allocating a console.
///
/// The daemon is a `windows` subsystem binary (no console of its own, so
/// it does not open a black window at every login). The consequence is
/// that **every** console child it creates gets a NEW console -- and the
/// peers refresher calls `ssh` every 6 s, which became a cmd window
/// flashing nonstop on the user's screen. Complained about, and rightly:
/// it looks broken.
///
/// `CREATE_NO_WINDOW` solves it at the source. It does not get in the way
/// of what actually has a UI: for a graphical app (`mstsc`, for example)
/// the flag only prevents the console, not its windows.

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn child(prog: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(prog);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}
