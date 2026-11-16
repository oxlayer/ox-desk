// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! LAN KVM: sends keyboard and mouse to another machine over the network.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! The alternative to HID++ 0x1814. That one tells the device ITSELF to
//! switch Easy-Switch channel, which requires a paired receiver on the
//! target machine. With a single dongle -- which is where we ended up
//! after two dongles close together proved unstable -- pressing "send to
//! windows-pc" made the keyboard look for a receiver that does not exist
//! there, not find it, and come back. It was the protocol's correct
//! behavior solving the wrong problem.
//!
//! Here the device does not move: we capture locally (`EVIOCGRAB`) and
//! inject on the other end through the same input stream the remote
//! screen already uses.
//!
//! ## A child process, not a thread
//!
//! `oxdesk-kvm` runs as a separate process, and that is design, not
//! laziness: **killing the process releases the grab**, because the
//! kernel releases it when the fd closes. If the KVM were a `oxdesk` thread
//! and it hung, the keyboard would stay grabbed and the only way out would
//! be killing the whole agent -- which is the thing you use to fix the
//! others. A child process makes giving up a one-line operation.

use std::process::{Child, Stdio};
use std::sync::Mutex;

use crate::config::Config;

/// Who is holding the keyboard: a child of ours, or a process that
/// survived a daemon restart.
enum Process {
    /// Born in this `oxdesk` instance.
    Ours(Child),
    /// Found alive at startup and adopted. We only have the pid.
    Adopted(u32),
}

/// The live process and who it is sending to.
static CURRENT: Mutex<Option<(String, Process)>> = Mutex::new(None);

/// Does the process still exist?
fn alive(p: &mut Process) -> bool {
    match p {
        Process::Ours(c) => !matches!(c.try_wait(), Ok(Some(_))),
        // `/proc/<pid>` is the cheapest question there is, and it does not
        // depend on the process being our child -- `waitpid` would.
        //
        // Adoption only exists on Linux, because only there is there
        // capture. On the others this arm is unreachable.
        #[cfg(target_os = "linux")]
        Process::Adopted(pid) => std::path::Path::new(&format!("/proc/{pid}")).exists(),
        #[cfg(not(target_os = "linux"))]
        Process::Adopted(_) => false,
    }
}

/// Who the input is going to now, or `None` if it is here.
pub fn target() -> Option<String> {
    let mut g = CURRENT.lock().ok()?;
    // Reap the child before answering: if it died (rescue via the two
    // Ctrls, target went down, network vanished), the input has already
    // come back to this machine, and saying otherwise would leave the
    // tray lying.
    if let Some((_, p)) = g.as_mut() {
        if !alive(p) {
            *g = None;
        }
    }
    g.as_ref().map(|(n, _)| n.clone())
}

/// Brings the input back to this machine. Idempotent.
pub fn release() -> Result<(), String> {
    let mut g = CURRENT.lock().map_err(|_| "KVM state poisoned")?;
    if let Some((name, p)) = g.take() {
        match p {
            Process::Ours(mut c) => {
                // On Windows the child ends its grab when its stdin closes
                // (`oxdesk-kvm`'s Windows `main`), and that path releases
                // the keys still held on the other machine. Killing skips
                // it. So: close, give it a moment, and kill only if it did
                // not go.
                if c.stdin.take().is_some() {
                    let t0 = std::time::Instant::now();
                    while t0.elapsed() < std::time::Duration::from_millis(1500) {
                        if matches!(c.try_wait(), Ok(Some(_))) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
                let _ = c.kill();
                let _ = c.wait();
            }
            // Not our child: std's `kill` does not reach it, so it goes by
            // signal. SIGTERM and not SIGKILL on purpose -- the KVM
            // releases the stuck keys on the other side before exiting,
            // and SIGKILL would skip that, leaving Shift held down on the
            // other machine.
            #[cfg(target_os = "linux")]
            Process::Adopted(pid) => unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            },
            #[cfg(not(target_os = "linux"))]
            Process::Adopted(_) => {}
        }
        say!("kvm: input back (it was on {name})");
    }
    Ok(())
}

/// Sends the input to `host`. Releases the previous target, if any.
///
/// Order matters: we release the previous one BEFORE trying the new one.
/// Two `EVIOCGRAB` on the same device do not coexist, and the second would
/// fail with "Device or resource busy" -- leaving the input stuck on the
/// old destination and the error message talking about something else.
pub fn send(cfg: &Config, host: &str) -> Result<String, String> {
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' is not in the config"))?;
    // Sending to the local machine = bringing it back. Same "is it here?"
    // test focus.rs uses, and not just the `local` flag: it is optional in
    // the config and not every host declares it, so trusting only it would
    // make "Trazer para linux-pc" try to open a KVM pointing at
    // ourselves.
    let here = h.local.unwrap_or(false)
        || matches!(h.addr.as_str(), "127.0.0.1" | "localhost" | "::1");
    if here {
        return release().map(|()| "input local".to_string());
    }
    let sc = h
        .screen
        .as_ref()
        .ok_or_else(|| format!("'{host}' has no `[hosts.screen]`: no port to send input to"))?;

    // macOS has injected input since 04/11 (`input_mac.rs`, CGEvent). The
    // refusal that used to be here went with it.
    //
    // What we CANNOT check from here is whether the host there has the
    // Accessibility permission: without it `CGEventPost` accepts the event
    // and silently drops it. Whoever warns is the host itself, at startup,
    // in its log -- and that is where to look when the KVM "does nothing".

    release()?;

    // Can this machine be the source?
    //
    // Linux captures with `EVIOCGRAB`, and Windows with low-level hooks
    // (spec 024). On Windows that only means something when the devices
    // are on THIS machine's receiver: otherwise the keyboard in front of
    // the person is being forwarded here by another machine, and "send it
    // to X" means "whoever holds it, let go" -- the source has to act.
    //
    // Before spec 024 Windows always took the second branch. With the
    // only dongle plugged into Windows, the click asked the Linux machine
    // to release a grab that did not exist, and nothing moved.
    //
    // The Mac still cannot capture; it always asks.
    let can_capture = cfg!(target_os = "linux") || (cfg!(windows) && crate::hidpp::devices_here());
    if !can_capture {
        return ask_to_release(h).map(|()| format!("asked {host} to take the input back"));
    }

    // Bring up the host on the other end first. It is the one receiving
    // the input stream; without it up, `oxdesk-kvm` refuses to connect
    // -- and refusing is the right behavior, because grabbing the keyboard
    // with no destination would leave the person without a keyboard AND
    // without anywhere to go.
    crate::screen::wake_host(h, sc.port)
        .map_err(|e| format!("could not bring up {host}'s host: {e}"))?;

    // `port + 1`: the host's INPUT port, not the video one.
    //
    // The host serves one video client at a time. While the KVM connected
    // to the video port, it took the viewer's slot -- the input arrived
    // and was applied, but the screen the person was looking at froze,
    // and the obvious conclusion was "it did not work". Separate ports
    // also keep the target machine from encoding video for a client that
    // never draws anything.
    let dest = format!("{}:{}", h.addr, sc.port + 1);
    // On Windows the address is resolved HERE and handed over ready, like
    // `probe::ssh_addr_opts` does for ssh, with the last known address when
    // mDNS flaps: a child resolving `.local` on its own died with "host not
    // found" (os error 11001) in one of those windows. Linux keeps passing
    // the name -- `adopt` reads it back from the command line.
    let connect_to = if cfg!(windows) {
        match crate::probe::resolve_with_memory(&h.addr, sc.port + 1) {
            Some((sa, false)) => sa.to_string(),
            Some((sa, true)) => {
                say!("kvm: {} did not resolve right now; using its last address {}", h.addr, sa.ip());
                sa.to_string()
            }
            None => {
                return Err(format!(
                    "'{}' does not resolve, and no earlier address is known. Nothing was grabbed.",
                    h.addr
                ))
            }
        }
    } else {
        dest.clone()
    };

    // Child output to a file, not to /dev/null.
    //
    // The first version threw stdout and stderr away, like the screen
    // client launch does. But that one opens a WINDOW: if it dies, the
    // person sees it. This one opens nothing -- the only sign that it
    // failed would be the keyboard staying here, without a line saying why.
    let log_path = crate::sys::state_dir().join("oxdesk-kvm.log");
    let out = std::fs::File::create(&log_path)
        .map_err(|e| format!("could not open {}: {e}", log_path.display()))?;
    let err = out
        .try_clone()
        .map_err(|e| format!("could not duplicate the descriptor: {e}"))?;

    // A Linux target applies its own key repeat (Mutter repeats injected
    // keys), so sending the kernel's too would double it. On Windows it is
    // the opposite: `SendInput` repeats nothing, and without the kernel's
    // holding backspace deletes a single character.
    // Next to this binary, not through PATH: a systemd user service does
    // not inherit `~/.local/bin`, and the failure only showed up on a
    // freshly booted machine. See `sys::own_bin`.
    let kvm_bin = crate::sys::own_bin("oxdesk-kvm");
    let mut cmd = crate::sys::child(&kvm_bin.to_string_lossy());
    cmd.arg(&connect_to);
    if h.os.as_deref() == Some("linux") {
        cmd.arg("--no-repeat");
    }
    // The keys that bring the input back are the keys that sent it
    // (spec 023). Passed only when they are not the default, so a pair of
    // binaries from before the flag keeps working with the default.
    let chord = cfg.input.hotkey.chord();
    if chord != crate::config::CHORDS[0].1 {
        cmd.arg("--rescue").arg(format!("{},{}", chord.0, chord.1));
    }
    // Every keystroke goes through this connection, so the host on the
    // other end is checked against its pinned identity, like the screen
    // viewer's (spec 017). Without a pin the KVM still connects and warns
    // in its log, which is how it always worked.
    match crate::pairing::pin_for(h, &crate::screen::local_host_bin()) {
        Ok(pin) => {
            cmd.env("OXDESK_PIN", pin);
        }
        Err(e) => say!("kvm: no pinned identity for {host} ({e}); connecting unchecked"),
    }
    // A pipe on Windows, held open for as long as the input is away:
    // closing it is how `release` ends the grab cleanly (see there). On
    // Linux the grab ends by signal, as before.
    let stdin = if cfg!(windows) { Stdio::piped() } else { Stdio::null() };
    let mut child = cmd
        .stdin(stdin)
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", kvm_bin.display()))?;

    // Confirm it SURVIVED, not just that it was born.
    //
    // `spawn` returns Ok as soon as the process exists -- before it
    // connects and before it grabs. Reporting success there would make the
    // tray say "sent to windows-pc" while the child had already died
    // because the target refused the connection or the /dev/input ACL was
    // missing. The state would fix itself on the next `target()`, but the
    // user would already have read the lie and be waiting for the keyboard
    // to change machines.
    //
    // It used to be a fixed 500 ms, read as "connect + grab on a LAN".
    // Name resolution alone took longer than that on Windows, the child
    // died AFTER the check, and the panel said "input going to ..." about
    // a keyboard that never left (spec 024). So the child's own word
    // decides: it prints "grabbed" only after the hooks or the grab hold.
    // Exited first: its last line is the reason. Neither within the
    // deadline: it is killed, because a grab that lands after we answered
    // "failed" would take the keyboard from someone who was told it stayed.
    let last_line = |path: &std::path::Path| {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .last()
            .unwrap_or("no detail in the log")
            .trim()
            .to_string()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the KVM exited ({status}) before grabbing: {}", last_line(&log_path)));
        }
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.lines().any(|l| l.trim_start().starts_with("grabbed")) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the KVM did not grab within 8 s, so it was stopped: {}", last_line(&log_path)));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    match CURRENT.lock() {
        Ok(mut g) => *g = Some((host.to_string(), Process::Ours(child))),
        Err(_) => return Err("KVM state poisoned".into()),
    }
    say!("kvm: input going to {host} ({dest})");
    Ok(dest)
}

/// Asks another machine's `oxdesk` to release its KVM.
///
/// Over SSH and `oxdesk kvm-release`, by the same path and for the same
/// reasons as the clipboard: the API listens only on 127.0.0.1, and SSH is
/// already the trusted channel between the machines.
///
/// It is not an error if the source is not sending anything -- releasing
/// what does not exist is a no-op, and the click still means the same thing.
fn ask_to_release(h: &crate::config::HostCfg) -> Result<(), String> {
    let dest = h.ssh.clone().ok_or_else(|| {
        format!("'{}' has no `ssh` in the config: there is no way to ask it anything", h.name)
    })?;
    // The new verb first, the one from before the rename second. The peer
    // runs its OWN build of oxdesk, and a peer we cannot deploy -- the Mac
    // without a screen -- only understands the old one. Retrying costs an
    // SSH round trip on a machine that is behind, and not retrying costs
    // the input staying where it is, which is the one failure this path
    // exists to prevent.
    let mut last = String::new();
    for verb in ["kvm-release", "kvm-soltar"] {
        let out = crate::sys::child("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &dest])
            .arg(remote_release_command(h, verb))
            .output()
            .map_err(|e| format!("ssh to {}: {e}", h.name))?;
        if out.status.success() {
            return Ok(());
        }
        last = String::from_utf8_lossy(&out.stderr).trim().to_string();
    }
    Err(format!("{} refused to release: {last}", h.name))
}

/// Same path rule as the clipboard: by OS, not by PATH.
fn remote_release_command(h: &crate::config::HostCfg, verb: &str) -> String {
    if h.os.as_deref() == Some("windows") {
        format!(r#"& "$env:LOCALAPPDATA\oxdesk\oxdesk.exe" {verb}"#)
    } else {
        format!("~/.local/bin/oxdesk {verb}")
    }
}

/// `oxdesk kvm-release`: tells the local daemon to give the input back.
///
/// Goes through the local API for the same reason as `clip-set`: the KVM
/// state lives in the daemon, and a separate CLI reaches neither the child
/// process nor `CURRENT`.
pub fn cli_release(port: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    let request = "POST /api/kvm/release HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        eprintln!("kvm-release: the local oxdesk did not answer on port {port}");
        return std::process::ExitCode::FAILURE;
    };
    if s.write_all(request.as_bytes()).is_err() {
        eprintln!("kvm-release: failed to write the request");
        return std::process::ExitCode::FAILURE;
    }
    let mut r = String::new();
    let _ = s.read_to_string(&mut r);
    if r.starts_with("HTTP/1.1 200") {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("kvm-release: {}", r.lines().next().unwrap_or("no response"));
        std::process::ExitCode::FAILURE
    }
}

/// Adopts a `oxdesk-kvm` that survived a daemon restart.
///
/// **This used to kill it.** The practical effect was brutal: the user was
/// working on the other machine, a deploy restarted `oxdesk`, and the
/// keyboard came back to Ubuntu on its own in the middle of a sentence. He
/// reported exactly that -- "do nada voltou para ca teclado e mouse, nao
/// estamos fazendo por lan agora?" ("out of nowhere keyboard and mouse
/// came back here, aren't we doing it over LAN now?").
///
/// Killing was defensible when the KVM was new: a process grabbing the
/// keyboard with no owner is scary. But the owner comes back in seconds,
/// and the alternative is yanking the input from someone who is using it.
/// Adopting is the right thing.
///
/// The target comes from the process's own command line (`<host>:<port>`),
/// and port-1 maps back to the config host -- the KVM connects to
/// `port + 1`.
///
/// Requires `KillMode=process` in systemd: without it `restart` takes down
/// the whole cgroup and nobody is left to adopt.
#[cfg(target_os = "linux")]
pub fn adopt(cfg: &Config) {
    let out = std::process::Command::new("pgrep")
        .args(["-a", "-x", "oxdesk-kvm"])
        .output();
    let Ok(o) = out else { return };
    let text = String::from_utf8_lossy(&o.stdout);
    let Some(line) = text.lines().next() else { return };

    // "12345 oxdesk-kvm windows-pc.local:9778"
    let mut fields = line.split_whitespace();
    let Some(pid) = fields.next().and_then(|p| p.parse::<u32>().ok()) else { return };
    let Some(dest) = fields.nth(1) else { return };
    let Some((address, port)) = dest.rsplit_once(':') else { return };
    let Ok(port) = port.parse::<u16>() else { return };

    let name = cfg
        .hosts
        .iter()
        .find(|h| {
            h.addr == address && h.screen.as_ref().is_some_and(|s| s.port + 1 == port)
        })
        .map(|h| h.name.clone())
        .unwrap_or_else(|| address.to_string());

    if let Ok(mut g) = CURRENT.lock() {
        *g = Some((name.clone(), Process::Adopted(pid)));
        crate::say!("kvm: adopted process {pid}, which is sending input to {name}");
    }
}

/// On the other systems there is no capture, so nothing to adopt.
#[cfg(not(target_os = "linux"))]
pub fn adopt(_cfg: &Config) {}

#[cfg(test)]
mod release_cmd_tests {
    use super::*;
    use crate::config::HostCfg;

    fn host(os: &str) -> HostCfg {
        let toml = format!("name = \"peer\"\naddr = \"peer.local\"\nos = \"{os}\"\n");
        toml::from_str(&toml).expect("fixture must parse")
    }

    /// The verb is substituted, not appended: the retry sends a second
    /// command, not a command with two verbs.
    #[test]
    fn the_verb_goes_into_the_command_for_each_os() {
        let posix = remote_release_command(&host("linux"), "kvm-release");
        assert_eq!(posix, "~/.local/bin/oxdesk kvm-release");
        let win = remote_release_command(&host("windows"), "kvm-release");
        assert!(win.contains("oxdesk.exe\" kvm-release"), "{win}");
        assert!(!win.contains("kvm-soltar"), "{win}");
    }

    /// The old verb has to still be expressible, because that is the
    /// retry: a peer running a build from before the rename -- the Mac,
    /// which cannot be deployed while it has no screen -- answers to
    /// nothing else, and the input would stay where it is.
    #[test]
    fn the_old_verb_is_still_expressible_for_the_retry() {
        assert_eq!(
            remote_release_command(&host("linux"), "kvm-soltar"),
            "~/.local/bin/oxdesk kvm-soltar"
        );
    }
}
