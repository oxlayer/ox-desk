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

    // Can this machine capture?
    //
    // Only Linux has capture (`EVIOCGRAB`). On a Windows or Mac, clicking
    // "Mandar para linux-pc" cannot mean "become the source" -- it
    // means "I want the input THERE", and whoever has to act is whoever is
    // holding the keyboard now: the source.
    //
    // Without this the click failed silently and the only way back was the
    // two-Ctrl rescue. The user reported exactly that: "tentei clicar no
    // tray do windows para mandar para linux-pc e nao voltou, so
    // funcionou apertando dois ctrl" ("I tried clicking the Windows tray
    // to send to linux-pc and it did not come back, it only worked
    // pressing two ctrl").
    if !cfg!(target_os = "linux") {
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
    let mut cmd = crate::sys::child("oxdesk-kvm");
    cmd.arg(&dest);
    if h.os.as_deref() == Some("linux") {
        cmd.arg("--sem-repeticao");
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("could not run oxdesk-kvm: {e}"))?;

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
    // 500 ms covers connect + grab on a LAN; the happy path does not feel
    // it, and the sad path gets the real error message.
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(Some(status)) = child.try_wait() {
        let reason = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .lines()
            .last()
            .unwrap_or("no detail in the log")
            .trim()
            .to_string();
        return Err(format!("the KVM exited ({status}) before grabbing: {reason}"));
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
/// Over SSH and `oxdesk kvm-soltar`, by the same path and for the same
/// reasons as the clipboard: the API listens only on 127.0.0.1, and SSH is
/// already the trusted channel between the machines.
///
/// It is not an error if the source is not sending anything -- releasing
/// what does not exist is a no-op, and the click still means the same thing.
fn ask_to_release(h: &crate::config::HostCfg) -> Result<(), String> {
    let dest = h.ssh.clone().ok_or_else(|| {
        format!("'{}' has no `ssh` in the config: there is no way to ask it anything", h.name)
    })?;
    let out = crate::sys::child("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", &dest])
        .arg(remote_release_command(h))
        .output()
        .map_err(|e| format!("ssh to {}: {e}", h.name))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} refused to release: {}",
            h.name,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Same path rule as the clipboard: by OS, not by PATH.
fn remote_release_command(h: &crate::config::HostCfg) -> String {
    if h.os.as_deref() == Some("windows") {
        r#"& "$env:LOCALAPPDATA\oxdesk\oxdesk.exe" kvm-soltar"#.into()
    } else {
        "~/.local/bin/oxdesk kvm-soltar".into()
    }
}

/// `oxdesk kvm-soltar`: tells the local daemon to give the input back.
///
/// Goes through the local API for the same reason as `clip-set`: the KVM
/// state lives in the daemon, and a separate CLI reaches neither the child
/// process nor `CURRENT`.
pub fn cli_release(port: u16) -> std::process::ExitCode {
    use std::io::{Read, Write};
    let request = "POST /api/kvm/release HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        eprintln!("kvm-soltar: the local oxdesk did not answer on port {port}");
        return std::process::ExitCode::FAILURE;
    };
    if s.write_all(request.as_bytes()).is_err() {
        eprintln!("kvm-soltar: failed to write the request");
        return std::process::ExitCode::FAILURE;
    }
    let mut r = String::new();
    let _ = s.read_to_string(&mut r);
    if r.starts_with("HTTP/1.1 200") {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("kvm-soltar: {}", r.lines().next().unwrap_or("no response"));
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
