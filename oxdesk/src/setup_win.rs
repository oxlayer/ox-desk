// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Setup measurement and repair, on Windows.
//!
//! Spec: ../../specs/011-onboarding.md
//!
//! Almost everything here revolves around **one** boundary: the session.
//! A service is born in session 0, and from there there is no desktop to
//! duplicate, `SendInput` is out of reach and the clipboard is always
//! empty. That is why the daemon and the screen host start through a
//! scheduled task with `/IT`, and why the wizard is not satisfied with
//! seeing that the task exists -- it checks its `LogonType`.
//!
//! ## Why XML and not `schtasks /query /v`
//!
//! The tabular output of `schtasks` is **localized**: on a pt-BR Windows
//! the field is "Modo de logon: Somente interativo". Searching for
//! "Interactive" there would work on this machine and fail on the next.
//! `/xml` returns `<LogonType>InteractiveToken</LogonType>`, which is a
//! schema name and does not change with the language. Same lesson
//! `unit_state` learned reading "ESTADO" instead of "STATE".

use crate::config::Config;
use crate::setup::{State, Check};
use crate::setup_common::run_timed;
use std::path::PathBuf;

const BINS: [&str; 7] = [
    "oxdesk.exe",
    "oxdesk-agent.exe",
    "oxdesk-host.exe",
    "oxdesk-view.exe",
    "oxdesk-bridge.exe",
    "oxdesk-kvm.exe",
    "oxdesk-ota.exe",
];

fn bin_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
    PathBuf::from(base).join("oxdesk")
}

pub fn measure(cfg: &Config) -> Vec<Check> {
    vec![
        binaries(),
        crate::setup_common::this_machine_config(cfg),
        task("daemon", "oxdesk"),
        task("screen-task", "oxdesk-host"),
        crate::setup_common::clipboard(),
        sshd(),
        admin_key(),
        crate::setup_common::peers(cfg),
        virtual_display(),
        screen_saver(),
        ota(),
    ]
}

fn ota_token() -> PathBuf {
    bin_dir().join("ota-token")
}

fn ota() -> Check {
    crate::setup_common::ota(&bin_dir().join("oxdesk-ota.exe"), &ota_token(), clock())
}

/// The scheduled task that wakes `oxdesk-ota` up.
fn clock() -> Result<String, String> {
    let m = task("ota", "oxdesk-ota");
    if m.state == crate::setup::State::Ok {
        Ok(m.detail)
    } else {
        Err(m.detail)
    }
}

fn binaries() -> Check {
    let dir = bin_dir();
    let absent: Vec<&str> = BINS.iter().copied().filter(|b| !dir.join(b).exists()).collect();
    if !absent.is_empty() {
        return Check::missing("binaries", format!("missing: {}", absent.join(", ")));
    }
    // Existing on disk is not being live. See
    // `setup_common::stamp_verdict`: a deploy can install the new file
    // and nobody runs it.
    if let Some(p) = crate::setup_common::daemon_on_disk(&dir.join("oxdesk.exe")) {
        return Check::missing("binaries", p);
    }
    Check::ok("binaries", format!("{} binaries in {}", BINS.len(), dir.display()))
}

/// Does the task exist AND run in the interactive session?
fn task(id: &str, name: &str) -> Check {
    // Failing here means "does not exist", not "I don't know".
    //
    // The first version tried to tell them apart by the error TEXT
    // ("annot find" / "ão foi poss") and I would have found out late that
    // this Windows is pt-BR: the real message is "ERRO: O sistema não
    // pode encontrar o arquivo especificado", and the accents even arrive
    // escaped through the SSH path. Treating any failure as absence is
    // right in practice -- `schtasks /query` on a task that exists does
    // not fail -- and the OS message goes along for the rare case.
    let xml = match run_timed("schtasks", &["/query", "/tn", name, "/xml", "ONE"], 10) {
        Ok(x) => x,
        Err(e) => return Check::missing(id, format!("could not find the task \"{name}\" ({e})")),
    };
    // `/xml` comes out in UTF-16 and `run_timed` reads it as lossy UTF-8,
    // so the text arrives sprinkled with NULs. Stripping the zeros is
    // cheaper (and more robust) than decoding: what we want is a
    // substring search.
    let clean: String = xml.chars().filter(|c| *c != '\0').collect();
    if !clean.contains("InteractiveToken") {
        return Check::missing(
            id,
            format!("\"{name}\" exists but is NOT interactive: it would land in session 0"),
        );
    }
    Check::ok(id, format!("\"{name}\" exists and runs in the interactive session"))
}

fn sshd() -> Check {
    match crate::sys::unit_state("sshd").as_str() {
        "active" => Check::ok("sshd", "the sshd service is running"),
        "inactive" => Check::missing("sshd", "the sshd service exists and is stopped"),
        other => Check::missing("sshd", format!("sshd: {other}")),
    }
}

fn admin_key() -> Check {
    // Windows OpenSSH ignores an administrator's ~/.ssh/authorized_keys
    // and reads ONLY this file. Putting the key in the "obvious" place
    // fails with "Permission denied" and no clue.
    let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let p = PathBuf::from(base).join("ssh").join("administrators_authorized_keys");

    // Existing and BEING ABLE TO READ are different questions here, and
    // the difference bit me on the first measurement: the file has 221
    // bytes and the wizard announced "not found".
    //
    // The correct ACL for this file is SYSTEM + Administrators and no one
    // else -- sshd **refuses** the file if it is looser. The daemon
    // starts through an `/IT` task at logon, with the UAC-filtered token,
    // where the Administrators SID is deny-only: the read fails. (Over
    // SSH the same account reads it, because OpenSSH hands out a full
    // token. That asymmetry is what hid the error.)
    //
    // So: `metadata` answers "exists" without reading the content, and
    // the key count only comes out when it can.
    let exists = std::fs::metadata(&p);
    match (exists, std::fs::read_to_string(&p)) {
        (Ok(_), Ok(text)) => {
            let n = text.lines().filter(|l| l.trim_start().starts_with("ssh-")).count();
            Check::when(
                "admin-key",
                n > 0,
                format!("{n} key(s) in {}", p.display()),
                format!("{} exists and holds no key at all", p.display()),
            )
        }
        // Exists and we cannot read it: that is the EXPECTED state on a
        // correctly configured machine. Marking it missing here would
        // flag a defect precisely in the correct configuration -- and a
        // wizard that gets the good case wrong is a wizard the person
        // learns to ignore. What really checks the content is the
        // `pares` step of the OTHER machines, which come in through here.
        (Ok(m), Err(_)) => Check::ok(
            "admin-key",
            format!(
                "exists ({} bytes); only administrators can read it, so the keys cannot be counted from here",
                m.len()
            ),
        ),
        (Err(e), _) if e.kind() == std::io::ErrorKind::NotFound => {
            Check::missing("admin-key", format!("could not find {}", p.display()))
        }
        (Err(e), _) => Check::error("admin-key", format!("{}: {e}", p.display())),
    }
}

fn virtual_display() -> Check {
    let count_cmd = "@(Get-PnpDevice -Class Monitor | Where-Object Present).Count";
    match run_timed("powershell", &["-NoProfile", "-Command", count_cmd], 15) {
        Ok(output) => crate::setup_common::monitors_verdict("virtual-display", &output),
        Err(e) => Check::error("virtual-display", format!("could not count the monitors: {e}")),
    }
}

/// Writes the installer embedded in the binary and runs it as
/// administrator. A new machine has no clone of the repo.
fn install_virtual_display() -> Result<String, String> {
    const SCRIPT: &str = include_str!("../setup/install-virtual-display.ps1");
    let target = bin_dir().join("install-virtual-display.ps1");
    std::fs::write(&target, SCRIPT).map_err(|e| format!("write {target:?}: {e}"))?;
    let elevate = format!(
        "Start-Process powershell -Verb RunAs -Wait -ArgumentList \
         '-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"'",
        target.display()
    );
    let _ = run_timed("powershell", &["-NoProfile", "-Command", &elevate], 180);
    let m = virtual_display();
    match m.state {
        State::Ok => Ok(m.detail),
        _ => Err(format!("{} -- accept the administrator prompt and measure again", m.detail)),
    }
}

fn screen_saver() -> Check {
    // Registry value names are not localized, the `reg query` output is
    // stable, and the question is small: will the screen saver kick in
    // and take the remote screen with it?
    let key = "HKCU\\Control Panel\\Desktop";
    let read = |value: &str| -> Option<String> {
        let t = run_timed("reg", &["query", key, "/v", value], 5).ok()?;
        t.lines()
            .find(|l| l.contains(value))
            .and_then(|l| l.split_whitespace().last())
            .map(str::to_string)
    };
    let active = read("ScreenSaveActive").unwrap_or_else(|| "?".into());
    let secure = read("ScreenSaverIsSecure").unwrap_or_else(|| "0".into());
    let timeout: u64 = read("ScreenSaveTimeOut").and_then(|v| v.parse().ok()).unwrap_or(0);

    if active == "0" || timeout == 0 {
        return Check::ok("screen-lock", "screen saver off: the remote screen will not drop on its own");
    }
    if secure == "0" {
        // A screen saver without a password does not lock the session,
        // and it is the lock -- not the screen saver -- that takes DXGI
        // down.
        return Check::ok(
            "screen-lock",
            format!("screen saver after {}min, no lock: DXGI keeps working", timeout / 60),
        );
    }
    Check::missing(
        "screen-lock",
        format!("locks after {}min: the remote screen dies with E_ACCESSDENIED", timeout / 60),
    )
}

// ------------------------------------------------------------------ repairs

pub fn repair(id: &str, _cfg: &Config) -> Result<String, String> {
    match id {
        "config" => crate::setup_common::write_config(),
        "clipboard" => crate::setup_common::test_clipboard(),
        "daemon" => create_task(
            "daemon",
            "oxdesk",
            &bin_dir().join("oxdesk.exe").to_string_lossy(),
            &["/sc", "onlogon"],
        ),
        // `once` at 00:00 has already passed: the task does not run on
        // its own, only when someone asks for the screen (`schtasks
        // /run`). The host capturing with nobody watching would waste GPU
        // and power.
        "screen-task" => create_task(
            "screen-task",
            "oxdesk-host",
            &bin_dir().join("oxdesk-host.exe").to_string_lossy(),
            &["/sc", "once", "/st", "00:00"],
        ),
        "ota" => install_clock(),
        "virtual-display" => install_virtual_display(),
        _ => Err(format!("'{id}' has no automatic fix on this machine")),
    }
}

/// Creates the hourly task and checks once.
///
/// The task runs the `.exe` **without arguments**. It is not sloppiness:
/// here `oxdesk-ota` already refuses to swap binaries on its own (the
/// Authenticode key does not leave the Ubuntu box -- specs/008), so the
/// default mode is already "check and report". And passing an argument
/// in `/tr` would require nested quotes, which have already broken this
/// bridge three times.
fn install_clock() -> Result<String, String> {
    let exe = bin_dir().join("oxdesk-ota.exe");
    create_task("ota", "oxdesk-ota", &exe.to_string_lossy(), &["/sc", "hourly"])?;
    let first = run_timed(&exe.to_string_lossy(), &["--check"], 60);
    Ok(match first {
        Ok(s) => format!("tarefa criada; primeira checagem: {}", s.trim()),
        Err(e) => format!("task created, but the first check failed: {e}"),
    })
}

fn create_task(id: &str, name: &str, exe: &str, when: &[&str]) -> Result<String, String> {
    if !std::path::Path::new(exe).exists() {
        return Err(format!("could not find {exe}: run the deploy first"));
    }
    let mut args: Vec<&str> = vec!["/create", "/tn", name, "/tr", exe];
    args.extend_from_slice(when);
    // `/it` is the whole point of this function. `/f` overwrites a
    // previous task -- including one created without `/it`, which is the
    // case we are fixing.
    args.extend_from_slice(&["/it", "/f"]);
    // The exit code is 0 with a WARNING when /ST is in the past: expected
    // and wanted. A real error comes with code != 0, and `run_timed`
    // already turns it into Err.
    run_timed("schtasks", &args, 20)?;
    // Measure afterwards, do not trust the code: I have seen `schtasks`
    // return 0 while creating the task without the interactive token when
    // the machine's policy does not allow it.

    let m = task(id, name);
    if m.state == crate::setup::State::Ok {
        Ok(m.detail)
    } else {
        Err(format!("created, and the check says: {}", m.detail))
    }
}
