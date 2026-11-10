// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Windows lets a SYSTEM process do on the lock screen.
//!
//! Spec: ../../../specs/014-lock-screen.md
//!
//! Same spirit as `mac-probe`: prove the pieces before assembling the
//! pipeline. Here the question that decides the design is a single one and
//! **varies by Windows version** -- there are reports of the process dying
//! right after locking on Win 11 and reports of it working fine as SYSTEM.
//! It cannot be decided by reading.
//!
//!   oxdesk-probe-system --who        who am I, and on which desktop
//!   oxdesk-probe-system --capture    can I duplicate the screen from here?
//!   oxdesk-probe-system --launch      become SYSTEM on the Winlogon desktop
//!
//! ## The path we are proving
//!
//! The lock screen and the UAC dialog live on their own **desktop**,
//! `Winlogon`, inside the `WinSta0` window station. DXGI does not duplicate
//! any desktop other than the input one, and only a LocalSystem process can
//! attach to `Winlogon`. A user process -- which is what we are today, via
//! an `/IT` task -- cannot even look.
//!
//! The pattern Parsec and RustDesk use, and that `--launch` exercises:
//!
//! 1. find the console session's `winlogon.exe` (`WTSGetActiveConsoleSessionId`)
//! 2. open its token and duplicate it as a primary token
//! 3. `CreateProcessAsUserW` with `lpDesktop = "winsta0\\Winlogon"`
//!
//! The child is born SYSTEM, in the console session, already on the right
//! desktop.
//!
//! ## Why a probe and not the service right away
//!
//! Installing a service means touching `HKLM` and creating a permanent
//! SYSTEM process -- and Defender has already deleted this machine's
//! `oxdesk.exe` once (spec 008). Before asking for that, it is worth knowing
//! whether the path works: `--launch` achieves the same effect from a
//! `/ru SYSTEM` scheduled task, which the administrator creates and deletes
//! without leaving residue.


#[cfg(windows)]
mod win {
use std::ffi::c_void;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SecurityImpersonation,
    TokenPrimary, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES,
    TOKEN_ALL_ACCESS, TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP,
};
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetCurrentProcessId,
    OpenProcessToken, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION,
    PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
};
use windows::Win32::System::Threading::OpenProcess;

/// Prints and **records**.
///
/// The process launched by `--launch` is born without a console (on
/// purpose: a new console would open a black window on the lock screen).
/// Without a file, its output would go nowhere -- and its output is the
/// entire result of the experiment.
macro_rules! say {
    ($($a:tt)*) => {{
        let line = format!($($a)*);
        println!("{line}");
        let _ = std::fs::create_dir_all("C:\\ProgramData\\oxdesk");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true).append(true).open("C:\\ProgramData\\oxdesk\\probe-system.log")
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }};
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Name of the desktop this thread is attached to.
fn current_desktop() -> String {
    unsafe {
        let h = windows::Win32::System::StationsAndDesktops::GetThreadDesktop(
            windows::Win32::System::Threading::GetCurrentThreadId(),
        );
        let Ok(h) = h else { return "?".into() };
        object_name(h.0 as *mut c_void)
    }
}

fn object_name(h: *mut c_void) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut n = 0u32;
        let ok = GetUserObjectInformationW(
            windows::Win32::Foundation::HANDLE(h),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut c_void),
            (buf.len() * 2) as u32,
            Some(&mut n),
        );
        if ok.is_err() {
            return "?".into();
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..end])
    }
}

fn am_system() -> bool {
    // Cheap way, without a SID: SYSTEM has USERNAME=<machine>$ or SYSTEM
    // depending on the path. The reliable answer here is the token's user
    // name, but for a probe all that matters is telling "it's me" from
    // "it's SYSTEM", and USERNAME is enough.
    std::env::var("USERNAME").map(|u| u.eq_ignore_ascii_case("SYSTEM") || u.ends_with('$'))
        .unwrap_or(false)
}

fn this_session() -> u32 {
    unsafe {
        let mut s = 0u32;
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut s);
        s
    }
}

fn who() {
    say!("  pid {} session {}", unsafe { GetCurrentProcessId() }, this_session());
    say!("  console session: {}", unsafe { WTSGetActiveConsoleSessionId() });
    say!("  USERNAME={} (SYSTEM? {})", std::env::var("USERNAME").unwrap_or_default(), am_system());
    say!("  this thread's desktop: {}", current_desktop());
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0)) {
            Ok(h) => {
                // THE QUESTION. Input desktop "Winlogon" = screen locked
                // or UAC in front. "Default" = normal desktop. A user
                // process cannot open Winlogon at all -- the error here is
                // already the answer.
                say!("  INPUT desktop: {}", object_name(h.0 as *mut c_void));
                let _ = CloseDesktop(h);
            }
            Err(e) => say!("  INPUT desktop: could not open it ({e}) -- typical of a user process"),
        }
    }
}

/// Attaches this thread to the input desktop and tries to duplicate the output.
fn capture() {
    unsafe {
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0)) {
            Ok(h) => {
                let name = object_name(h.0 as *mut c_void);
                match SetThreadDesktop(h) {
                    Ok(()) => say!("  attached to the input desktop: {name}"),
                    Err(e) => say!("  did NOT attach to {name}: {e}"),
                }
            }
            Err(e) => say!("  could not open the input desktop: {e}"),
        }
    }
    say!("  this thread's desktop now: {}", current_desktop());

    // The real test: the same path as the real host.
    match oxdesk_stream::capture_win::Monitor::open(0) {
        Ok(m) => match m.duplicate() {
            Ok(t) => {
                let (lw, lh) = t.size();
                say!("  DUPLICATED: {lw}x{lh} -- capture works on this desktop");
            }
            Err(e) => say!("  duplicate FAILED: {e}"),
        },
        Err(e) => say!("  opening the monitor failed: {e}"),
    }
}

/// Follows the input desktop for N seconds, capturing on every round.
///
/// It exists because the first measurement contradicted itself: `LogonUI`
/// was on screen (that is, locked) and the probe read the input desktop as
/// `Default`. A single snapshot cannot tell "the reading is wrong" from "the
/// lock had not switched the desktop yet when I looked". A film can -- and
/// as a bonus it shows HOW LONG the switch takes, which is exactly what the
/// service will need to tolerate.
#[cfg(windows)]
fn follow(seconds: u64) {
    for i in 0..seconds {
        let (desktop, dup) = unsafe {
            match OpenInputDesktop(
                DESKTOP_CONTROL_FLAGS(0),
                false,
                DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0),
            ) {
                Ok(h) => {
                    let name = object_name(h.0 as *mut c_void);
                    // Re-attach ON EVERY ROUND: DXGI duplication is per
                    // desktop, and whoever does not re-attach keeps looking
                    // at the old desktop -- which is how a capturer "works"
                    // and delivers stale frames.
                    let _ = SetThreadDesktop(h);
                    let r = match oxdesk_stream::capture_win::Monitor::open(0) {
                        Ok(m) => match m.duplicate() {
                            Ok(t) => {
                                let (w, h2) = t.size();
                                format!("duplicated {w}x{h2}")
                            }
                            Err(e) => format!("did NOT duplicate: {}", first_line(&e.to_string())),
                        },
                        Err(e) => format!("no monitor: {}", first_line(&e.to_string())),
                    };
                    (name, r)
                }
                Err(e) => ("<not opened>".to_string(), format!("OpenInputDesktop: {e}")),
            }
        };
        say!("  [{i:02}s] input desktop={desktop:12} {dup}");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(windows)]
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).chars().take(90).collect()
}

/// Finds a process by name and returns its pid in the requested session.
fn pid_of(name: &str, session: u32) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let end = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(0);
                let exe = String::from_utf16_lossy(&e.szExeFile[..end]);
                if exe.eq_ignore_ascii_case(name) {
                    let mut s = 0u32;
                    if ProcessIdToSessionId(e.th32ProcessID, &mut s).is_ok() && s == session {
                        found = Some(e.th32ProcessID);
                        break;
                    }
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Enables a privilege on our own token.
fn enable_privilege(name: &str) -> bool {
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut luid = LUID::default();
        let n = w(name);
        let ok = LookupPrivilegeValueW(None, PWSTR(n.as_ptr() as *mut u16), &mut luid).is_ok();
        if ok {
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            let _ = AdjustTokenPrivileges(tok, false, Some(&tp), 0, None, None);
        }
        let _ = CloseHandle(tok);
        ok
    }
}

/// The "service dance": become SYSTEM on the console session's Winlogon desktop.
fn launch(rest: &[String]) {
    let session = unsafe { WTSGetActiveConsoleSessionId() };
    say!("  console session: {session}");

    // `winlogon.exe` is the target because it ALWAYS exists in the
    // interactive session, runs as SYSTEM, and exists even with no user
    // logged in -- which is exactly the lock screen state. `explorer` will
    // not do: it belongs to the user and goes away when nobody is logged in.
    let Some(pid) = pid_of("winlogon.exe", session) else {
        say!("  did not find winlogon.exe in session {session}");
        return;
    };
    say!("  winlogon.exe pid {pid}");

    // Without these, CreateProcessAsUser fails with ERROR_PRIVILEGE_NOT_HELD
    // and the message mentions no privilege at all.
    for p in ["SeDebugPrivilege", "SeIncreaseQuotaPrivilege", "SeAssignPrimaryTokenPrivilege"] {
        say!("  privilege {p}: {}", if enable_privilege(p) { "enabled" } else { "NO" });
    }

    unsafe {
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            say!("  could not open the winlogon process (need to be SYSTEM or have SeDebugPrivilege)");
            return;
        };
        let mut tok = HANDLE::default();
        if OpenProcessToken(proc, TOKEN_DUPLICATE | TOKEN_QUERY, &mut tok).is_err() {
            say!("  could not open winlogon's token");
            let _ = CloseHandle(proc);
            return;
        }
        let mut primary = HANDLE::default();
        // PRIMARY token, not an impersonation one: CreateProcessAsUser
        // refuses the impersonation one, and the documentation is explicit
        // -- "the token created by DuplicateToken cannot be used in
        // CreateProcessAsUser".
        if DuplicateTokenEx(tok, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut primary).is_err() {
            say!("  DuplicateTokenEx failed");
            let _ = CloseHandle(tok);
            let _ = CloseHandle(proc);
            return;
        }

        let me = std::env::current_exe().unwrap_or_default();
        let args: Vec<String> = rest.to_vec();
        let line = format!("\"{}\" {}", me.display(), args.join(" "));
        let mut line_w = w(&line);
        // `winsta0\Winlogon` is the whole point: it is the lock and UAC
        // desktop. `winsta0\default` would give the normal desktop.
        let mut desktop = w("winsta0\\Winlogon");

        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        let r = CreateProcessAsUserW(
            Some(primary),
            None,
            Some(PWSTR(line_w.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            None,
            None,
            &si,
            &mut pi,
        );
        match r {
            Ok(()) => say!("  LAUNCHED pid {} on desktop winsta0\\Winlogon", pi.dwProcessId),
            Err(e) => say!("  CreateProcessAsUser failed: {e}"),
        }
        let _ = CloseHandle(primary);
        let _ = CloseHandle(tok);
        let _ = CloseHandle(proc);
    }
}

pub fn run() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mode = a.first().map(String::as_str).unwrap_or("--who");
    say!("== oxdesk-probe-system {mode} ==");
    // Timestamp: this probe runs from a scheduled task, without a console,
    // and without the time there is no way to tell whether the output is
    // from this run or the previous one.
    say!("  {}", simple_clock());
    match mode {
        "--who" => who(),
        "--capture" => {
            who();
            capture();
        }
        "--launch" => launch(if a.len() > 1 { &a[1..] } else { &[] }),
        "--follow" => {
            who();
            let n = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(15);
            follow(n);
        }
        other => say!("  unknown mode: {other}"),
    }
}

/// Local time without pulling a dependency: the probe does not deserve a crate.
fn simple_clock() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}
}

#[cfg(windows)]
fn main() {
    win::run();
}

// Same pattern as mac-probe: on the wrong OS the binary exists and says what
// it is, instead of failing to compile. The deploy builds ALL of the crate's
// bins on Ubuntu, so a bin that only compiles on Windows would break the
// whole deploy -- which is what happened.
#[cfg(not(windows))]
fn main() {
    eprintln!("this probe is for Windows: it measures what a SYSTEM process can reach on the lock screen");
}
