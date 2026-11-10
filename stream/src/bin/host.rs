// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Screen host: dispatches to this platform's implementation.
//!
//! Spec: ../../specs/010-remote-desktop.md
//!
//! One binary with the SAME name on every machine on purpose: whoever
//! wakes the host (the panel, over SSH) does not need to know whether the
//! other side has PipeWire or DXGI. What changes is the second argument --
//! video connector on Linux, monitor index on Windows -- because the unit
//! of "one screen" differs between the two, and inventing a common name
//! would hide that.
//!
//!   oxdesk-host [port] [connector|monitor]
//!
//! ## No console on Windows
//!
//! This binary starts from a scheduled task with `/IT`, in the interactive
//! session -- and a task that runs a console program opens a **visible
//! console window**, right in the middle of the screen you are watching.
//! The user saw it as "that terminal that opens when connecting to the
//! screen or switching the KVM".
//!
//! `oxdesk` already solved this with `windows_subsystem = "windows"`; here
//! it was missing. But dropping the console would also drop the log, which
//! is the tool we use most to diagnose this process -- so the program
//! redirects its own output to a file before writing the first line.

// No console. The log goes out through `redirect_output()`, below.
#![cfg_attr(windows, windows_subsystem = "windows")]

/// Sends stdout and stderr to the log file.
///
/// Has to run **before any `println!`**: Rust resolves the stdout handle
/// on the first write and keeps it, so swapping it later would have no
/// effect on what was already resolved.
///
/// The file is deliberately leaked (`into_raw_handle` without closing):
/// it must live as long as the process, and tying it to a variable would
/// only create the chance of someone dropping it.
#[cfg(windows)]
fn redirect_output() {
    use std::os::windows::io::{AsRawHandle, IntoRawHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};

    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(base).join("oxdesk");
    let _ = std::fs::create_dir_all(&dir);
    // Keep the previous session's log as `.1` (see oxdesk's wake command).
    let _ = std::fs::rename(dir.join("oxdesk-host.log"), dir.join("oxdesk-host.log.1"));
    let Ok(f) = std::fs::File::create(dir.join("oxdesk-host.log")) else {
        return;
    };
    let h = HANDLE(f.as_raw_handle() as _);
    unsafe {
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        let _ = SetStdHandle(STD_ERROR_HANDLE, h);
    }
    let _ = f.into_raw_handle();
}

fn main() -> anyhow::Result<()> {
    // Spec 017: the fingerprint other machines pin, read over SSH by the
    // panel. Answered BEFORE the Windows redirect, or it would land in the
    // log file instead of the SSH pipe.
    // `--digital` is what this answered to before the rename, and it is
    // invoked ON THE OTHER MACHINE over SSH by `oxdesk/src/pairing.rs`.
    // So the old name has to keep working for a caller we have not
    // updated, and the caller tries the new one first.
    if matches!(std::env::args().nth(1).as_deref(), Some("--fingerprint") | Some("--digital")) {
        println!("{}", oxdesk_stream::identity::load()?.fingerprint());
        return Ok(());
    }

    // Same reason `--check` and `--request` live here: macOS ties the
    // Screen Recording grant to THIS binary's designated requirement. A
    // fresh probe binary is refused with `-3801 The user declined TCCs`
    // (measured), while this one already holds the grant and keeps it
    // across rebuilds thanks to the local signing identity. See
    // specs/020-native-mac.md.
    #[cfg(target_os = "macos")]
    if std::env::args().nth(1).as_deref() == Some("--probe-native") {
        return oxdesk_stream::mac_native::probe().map_err(|e| anyhow::anyhow!(e));
    }

    #[cfg(windows)]
    redirect_output();

    let a: Vec<String> = std::env::args().skip(1).collect();

    // `--check` and `--request` live in THIS binary and not in the daemon
    // because macOS TCC ties the grant to the binary -- to its designated
    // requirement. `oxdesk` asking "do I have screen permission?" would get
    // a correct answer about the wrong program, and the setup wizard would
    // paint green a permission the host does not have.
    //
    // The output is ONE line per permission, `key: ok|missing`, because
    // the reader is code. See oxdesk/src/setup_mac.rs.
    //
    // The old spellings still answer. This binary and the wizard that
    // reads it ship in the same deploy, and the launchd job that passes
    // the flag is submitted fresh on every check rather than installed --
    // so the exposure is a partial deploy and a shell history, not a
    // stale plist. Cheap either way, and a flag this one rejects turns a
    // granted permission into "run the deploy".
    #[cfg(target_os = "macos")]
    if a.first().is_some_and(|v| {
        v == "--check" || v == "--request" || v == "--conferir" || v == "--pedir"
    }) {
        let request = a[0] == "--request" || a[0] == "--pedir";
        if request {
            // Shows the system dialog. Only works when launched from the
            // graphical session: over SSH there is no dialog to show and
            // the call returns `false` without anyone having seen anything.
            let _ = oxdesk_stream::input_mac::request_permissions();
        }
        // Measure AFTER requesting, and always through the preflight: the
        // request's return value is what the user just clicked, and in the
        // common case (dialog already answered some day) there is not even
        // a dialog. What answers "can we capture now?" is the preflight.
        // Keys and values are parsed by oxdesk/src/setup_mac.rs: changing
        // one side alone breaks the pair silently, which is exactly what
        // had happened -- this printed `acessibilidade:` while the wizard
        // looked for `accessibility:`, so a granted permission was
        // reported as "the installed host does not understand --conferir".
        println!(
            "screen-recording: {}",
            if oxdesk_stream::input_mac::has_screen_recording() { "ok" } else { "missing" }
        );
        println!(
            "accessibility: {}",
            if oxdesk_stream::input_mac::has_accessibility() { "ok" } else { "missing" }
        );
        return Ok(());
    }

    // `--bind` decides WHICH address to listen on, and the default is
    // still 0.0.0.0 -- no machine changes behavior without someone asking.
    //
    // `--bind overlay` exists for the machine that LEAVES HOME. The screen
    // channel encrypts but does not authenticate (see net.rs), so on
    // 0.0.0.0 on a hotel Wi-Fi it serves the screen and the keyboard to the
    // whole network. See specs/012.
    let bind = match a.iter().position(|v| v == "--bind") {
        Some(i) => match a.get(i + 1).map(|v| oxdesk_stream::bind::Mode::parse(v)) {
            Some(Ok(m)) => m,
            Some(Err(e)) => anyhow::bail!("{e}"),
            None => anyhow::bail!("--bind needs a value: todas | overlay | <address>"),
        },
        None => oxdesk_stream::bind::Mode::All,
    };
    match oxdesk_stream::bind::resolve(&bind, &oxdesk_stream::bind::local_addrs()) {
        Ok(ip) => oxdesk_stream::bind::set_listen_addr(ip),
        // Failing here is the design, not an accident: whoever asked for
        // overlay prefers a host that does not start to a host open to the
        // wrong network.
        Err(e) => anyhow::bail!("{e}"),
    }

    // Bandwidth cap. It exists for the session that leaves home: the
    // bottleneck stops being the LAN and becomes the home UPLOAD, and
    // 15 Mbps over a 10 Mbps upload does not give a bad picture -- it gives
    // a queue at the router, rising latency and a screen falling apart.
    // See specs/012.
    match oxdesk_stream::quality::cap_from_args(&a) {
        Ok(Some(bits)) => oxdesk_stream::quality::set_cap(bits),
        Ok(None) => {}
        Err(e) => anyhow::bail!("{e}"),
    }

    // Positionals come after the flag parsing, and filtered: without this
    // `--bind` would become the "port" and the host would come up on 9777
    // by a parsing accident.
    let pos: Vec<&String> = {
        let mut v = Vec::new();
        let mut skip = false;
        for x in &a {
            if skip { skip = false; continue; }
            if x == "--bind" || x == "--mbps" { skip = true; continue; }
            if x.starts_with("--") { continue; }
            v.push(x);
        }
        v
    };
    let port: u16 = pos.first().and_then(|v| v.parse().ok()).unwrap_or(9777);

    #[cfg(target_os = "linux")]
    {
        let connector = pos.get(1).map(|s| s.to_string()).unwrap_or_else(|| "DP-1".into());
        oxdesk_stream::host_linux::run(port, &connector)
    }

    #[cfg(windows)]
    {
        let monitor: u32 = pos.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        oxdesk_stream::host_win::run(port, monitor)
    }

    #[cfg(target_os = "macos")]
    {
        // Device name in avfoundation. An index does not work: the
        // numbering includes cameras and changes on its own (see `host_mac`).
        let device =
            pos.get(1).map(|s| s.to_string()).unwrap_or_else(|| "Capture screen 0".into());
        oxdesk_stream::host_mac::run(port, &device)
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (port, pos);
        anyhow::bail!(
            "this system does not know how to serve its own screen yet; here it \
             is the client -- use `oxdesk-view`"
        )
    }
}
