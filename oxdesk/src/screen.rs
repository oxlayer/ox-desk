//! Remote screen: we do not serve a screen, we dispatch the right client.
//!
//! Spec: ../specs/006-screen-share.md
//!
//! ## The decision (03/11, the user's): each OS's native one
//!
//! Each machine already knows how to be controlled, and through a
//! different protocol: GNOME speaks RDP through `gnome-remote-desktop`
//! (already active, 3389 exposed on the LAN, TLS and credentials set up),
//! Windows speaks native RDP, macOS speaks VNC through Screen Sharing.
//! There is no protocol all three serve out of the box.
//!
//! The alternative considered was a self-hosted RustDesk: the same
//! client on all three, and only outbound connections -- which would
//! matter on a Mac with MDM. It lost because it is a big external
//! dependency and a relay to maintain, while the native route starts no
//! new daemon anywhere.
//!
//! ## What this module does, and what it does not
//!
//! It does **not** implement a screen protocol. It answers two questions
//! -- "is that host's screen reachable?" and "is there a client here to
//! talk to it?" -- and, on request, launches the client. The hard part
//! (video, input, TLS) belongs to the programs that already exist.
//!
//! That is why `GET` is separate from `POST`: the UI needs to know whether
//! the button will work BEFORE the person clicks. A button that opens
//! nothing is worse than a disabled button with the reason written
//! beside it.

use crate::config::Config;
use serde::Serialize;
use std::path::PathBuf;
use std::process::Stdio;

/// What the person asked for in the UI, Parsec-style.
#[derive(Debug, Clone, Copy)]
pub struct Opts {
    /// Come in with keyboard and mouse. `false` = view only.
    pub control: bool,
    /// Bring the other machine's sound.
    pub audio: bool,
    /// Send the microphone from here to there.
    pub mic: bool,
}

impl Default for Opts {
    fn default() -> Self {
        // Controlling is the common case; sound and microphone are not,
        // because turning audio on without asking is the kind of surprise
        // that scares people in a meeting.
        Opts { control: true, audio: false, mic: false }
    }
}

/// What the chosen client can honor. The UI uses this to disable what is
/// pointless to tick, with the reason beside it.
#[derive(Debug, Serialize, Default)]
pub struct Caps {
    pub audio: bool,
    pub mic: bool,
    /// Can come in WITHOUT keyboard and mouse.
    pub view_only: bool,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub host: String,
    /// rdp | vnc
    pub kind: String,
    pub port: u16,
    /// The port answered just now.
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ms: Option<f64>,
    /// Chosen client, if we found one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// Command line that would be run -- so the UI can show it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    /// What this client can do.
    pub caps: Caps,
    /// Ready to click.
    pub ready: bool,
    /// Why it is not ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

/// Looks for an executable in PATH and in the directories the platform
/// points to. On Windows it also tries without `.exe`, because PATHEXT
/// resolves that for the shell but not for us.
fn find_program(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(crate::sys::screen_client_dirs());
    for d in dirs {
        let c = d.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

/// Replaces the markers in the client's template.
///
/// `{u_flag}` exists because freerdp wants `/u:name` as ONE argument, and
/// when there is no user the argument has to **disappear** -- passing an
/// empty `/u:` makes it prompt for a user and hang without a terminal.
/// Writes the `.rdp` to a temp file and returns the path.
///
/// A file and not an argument because some clients only accept it that
/// way -- and because sound and microphone go through it. A fixed name
/// per host: a new file on every click would fill temp for no reason.
fn write_rdp(template: &str, host: &str, port: u16, user: Option<&str>, opts: Opts) -> Option<String> {
    let user_line = user.map(|u| format!("username:s:{u}\n")).unwrap_or_default();
    let body = template
        .replace("{host}", host)
        .replace("{port}", &port.to_string())
        .replace("{user_line}", &user_line)
        // 0 = play here, 2 = play nowhere.
        .replace("{audiomode}", if opts.audio { "0" } else { "2" })
        .replace("{audiocapture}", if opts.mic { "1" } else { "0" });
    let safe: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let p = std::env::temp_dir().join(format!("oxdesk-{safe}.rdp"));
    std::fs::write(&p, body).ok()?;
    Some(p.to_string_lossy().into_owned())
}

fn render(args: &[&str], host: &str, port: u16, user: Option<&str>) -> Vec<String> {
    let user_at = user.map(|u| format!("{u}@")).unwrap_or_default();
    args.iter()
        .filter_map(|a| {
            if *a == "{u_flag}" {
                return user.map(|u| format!("/u:{u}"));
            }
            Some(
                a.replace("{host}", host)
                    .replace("{port}", &port.to_string())
                    .replace("{user_at}", &user_at),
            )
        })
        .collect()
}

fn plan_for(cfg: &Config, name: &str, opts: Opts) -> Option<Plan> {
    let h = cfg.hosts.iter().find(|x| x.name == name)?;
    let sc = h.screen.as_ref()?;
    // This machine is not controlled through a remote screen -- it is the
    // screen you are looking at.
    if h.is_local() {
        return None;
    }
    // Our protocol is QUIC, that is, **UDP**: a TCP probe would say
    // "closed" forever and the button would never light up. And its host
    // starts on demand, so not even being up is a prerequisite -- what
    // matters is having an SSH channel to start it.
    let (reachable, ms) = if sc.kind == "oxdesk-stream" {
        (h.ssh.is_some(), None)
    } else {
        crate::probe::tcp_probe(&h.addr, sc.port)
    };
    let user = sc.user.as_deref();

    let mut client = None;
    let mut command = None;
    let mut caps = Caps::default();
    for spec in crate::sys::screen_clients(&sc.kind) {
        let Some(found) = find_program(spec.prog) else { continue };
        let mut args: Vec<&str> = spec.args.to_vec();
        if opts.audio {
            if let Some(extra) = spec.audio {
                args.extend_from_slice(extra);
            }
        }
        if opts.mic {
            if let Some(extra) = spec.mic {
                args.extend_from_slice(extra);
            }
        }
        if !opts.control {
            if let Some(extra) = spec.view_only {
                args.extend_from_slice(extra);
            }
        }
        let file = spec.file.and_then(|m| write_rdp(m, &h.addr, sc.port, user, opts));
        let mut cmd = vec![found.to_string_lossy().into_owned()];
        cmd.extend(
            render(&args, &h.addr, sc.port, user)
                .into_iter()
                .map(|a| match &file {
                    Some(f) => a.replace("{file}", f),
                    None => a,
                })
                .collect::<Vec<_>>(),
        );
        caps = Caps {
            audio: spec.audio.is_some(),
            mic: spec.mic.is_some(),
            view_only: spec.view_only.is_some(),
        };
        client = Some(spec.prog.to_string());
        command = Some(cmd);
        break;
    }

    let blocker = if client.is_none() {
        let tried: Vec<&str> =
            crate::sys::screen_clients(&sc.kind).into_iter().map(|c| c.prog).collect();
        Some(if tried.is_empty() {
            format!("protocolo '{}' nao tem cliente conhecido aqui", sc.kind)
        } else {
            format!("sem cliente {} instalado aqui (procurei: {})", sc.kind, tried.join(", "))
        })
    } else if !reachable {
        Some(if sc.kind == "oxdesk-stream" {
            format!(
                "'{}' nao tem `ssh` na config: sem ele nao ha como levantar o host de tela la",
                h.name
            )
        } else {
            format!(
                "{}:{} nao respondeu -- a tela do host esta desligada ou bloqueada",
                h.addr, sc.port
            )
        })
    } else {
        None
    };

    Some(Plan {
        host: h.name.clone(),
        kind: sc.kind.clone(),
        port: sc.port,
        reachable,
        ms: ms.map(|v| (v * 10.0).round() / 10.0),
        ready: blocker.is_none(),
        client,
        command,
        caps,
        blocker,
    })
}

/// One plan per host that has `[hosts.screen]` and is not this one.
pub fn plans(cfg: &Config) -> Vec<Plan> {
    // In parallel: each plan does a TCP probe with a 1.5 s timeout, and in
    // series three offline hosts would cost 4.5 s of waiting when the
    // panel opens.
    std::thread::scope(|s| {
        let hs: Vec<_> = cfg
            .hosts
            .iter()
            .map(|h| {
                let name = h.name.clone();
                s.spawn(move || plan_for(cfg, &name, Opts::default()))
            })
            .collect();
        hs.into_iter().filter_map(|j| j.join().ok().flatten()).collect()
    })
}

/// Launches the client. Returns what was run, for the log and the UI.
pub fn launch(cfg: &Config, host: &str, opts: Opts) -> Result<String, String> {
    let plan = plan_for(cfg, host, opts)
        .ok_or_else(|| format!("host '{host}' nao tem `[hosts.screen]` na config"))?;
    if let Some(b) = &plan.blocker {
        return Err(b.clone());
    }
    // The screen host starts on demand, before the client tries to
    // connect. Capturing and encoding with nobody watching would only
    // burn GPU on the other person's machine.
    let mut pin = None;
    if plan.kind == "oxdesk-stream" {
        if let Some(h) = cfg.hosts.iter().find(|x| x.name == host) {
            if let Err(e) = wake_host(h, plan.port) {
                return Err(format!("nao consegui levantar o host de tela: {e}"));
            }
            pin = Some(crate::pairing::pin_for(h, &local_host_bin())?);
        }
    }

    let cmd = plan.command.ok_or("sem comando")?;
    let (prog, args) = cmd.split_first().ok_or("comando vazio")?;

    // `spawn` and not `output`: the screen client is interactive and
    // lives for minutes or hours. Waiting for it would block the server
    // thread. stdio is ignored so the child does not inherit ours and keep
    // a pipe open.
    let mut child = crate::sys::child(prog);
    if let Some(p) = &pin {
        // Spec 017: the viewer refuses a host whose identity is not this one.
        child.env("OXDESK_PIN", p);
    }
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nao consegui executar {prog}: {e}"))?;

    let line = cmd.join(" ");
    let mode = format!(
        "{}{}{}",
        if opts.control { "control" } else { "view only" },
        if opts.audio { " +audio" } else { "" },
        if opts.mic { " +mic" } else { "" }
    );
    say!("screen: opening {host} ({}, {mode}) -- {line}", plan.kind);
    Ok(line)
}

/// What the tab needs to talk to the bridge.
#[derive(serde::Serialize)]
pub struct Session {
    #[serde(rename = "porta")]
    pub port: u16,
    pub token: String,
}

/// Opens a session in a tab: wakes the host and starts
/// `oxdesk-bridge`.
///
/// Without the bridge installed it returns an error, and the page falls
/// back to the native viewer. See specs/016-session-and-visuals.md.
pub fn open_session(cfg: &Config, host: &str) -> Result<Session, String> {
    let plan = plan_for(cfg, host, Opts::default())
        .ok_or_else(|| format!("host '{host}' nao tem `[hosts.screen]` na config"))?;
    if let Some(b) = &plan.blocker {
        return Err(b.clone());
    }
    if plan.kind != "oxdesk-stream" {
        return Err(format!("'{host}' nao usa oxdesk-stream"));
    }
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' nao esta na config"))?;
    let name = if cfg!(windows) { "oxdesk-bridge.exe" } else { "oxdesk-bridge" };
    let bridge = find_program(name).ok_or("oxdesk-bridge nao instalada")?;
    wake_host(h, plan.port).map_err(|e| format!("nao consegui levantar o host de tela: {e}"))?;
    let pin = crate::pairing::pin_for(h, &local_host_bin())?;

    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|e| format!("token: {e}"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let mut child = crate::sys::child(&bridge.to_string_lossy())
        .arg(format!("{}:{}", h.addr, plan.port))
        .env("OXDESK_BRIDGE_TOKEN", &token)
        .env("OXDESK_PIN", &pin)
        // Telemetry (spec 017): the bridge records only if it gets the file.
        .env("OXDESK_PATH", crate::telemetry::path_to(h))
        .env("OXDESK_VERSION", env!("OXDESK_BUILD"))
        .env("OXDESK_HOST_OS", format!("{:?}", h.os()).to_lowercase())
        .envs(cfg.telemetry_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("nao consegui executar a ponte: {e}"))?;
    let output = child.stdout.take().ok_or("a ponte nao tem saida")?;
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(output), &mut line)
        .map_err(|e| format!("ler a ponte: {e}"))?;
    let port = line
        .trim()
        .strip_prefix("porta ")
        .and_then(|p| p.parse().ok())
        .ok_or("a ponte nao disse a porta")?;
    // The bridge exits on its own when the tab closes; this thread only
    // reaps the child.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    say!("screen: tab for {host} through the bridge on port {port}");
    Ok(Session { port, token })
}

/// This machine's own host binary, which knows this machine's identity.
/// A missing one only costs the self-authorization step (see `pairing`).
pub fn local_host_bin() -> PathBuf {
    let name = if cfg!(windows) { "oxdesk-host.exe" } else { "oxdesk-host" };
    find_program(name).unwrap_or_else(|| PathBuf::from(name))
}

/// Makes sure `oxdesk-host` is running on the other machine.
///
/// Idempotent on purpose: the button can be clicked twice, and a second
/// instance would compete for the same port and the same capture.
///
/// How to wake it depends on the target's OS, and the difference is not
/// cosmetic:
///
/// - **Unix**: `setsid nohup` detaches the process from the SSH session.
///   Without it, it dies along with the command that created it -- the
///   same trap as Windows killing the tree on disconnect (specs/001).
///   `setsid` is conditional because **macOS does not have it**: there
///   the whole line died with "command not found" before reaching nohup.
/// - **Windows**: a scheduled task with `/it`, and **not** a direct
///   launch. SSH lands in session 0, where Desktop Duplication does not
///   exist and `SendInput` reaches no desktop: the process would start,
///   would not fail, and would deliver a screen nobody is seeing. It is
///   the same session boundary that forces `oxdesk` to start via
///   `schtasks`.
pub(crate) fn wake_host(h: &crate::config::HostCfg, port: u16) -> Result<(), String> {
    let cmd = if crate::remote::is_powershell(h) {
        // No double quotes anywhere: three quoting bugs on this bridge
        // have already cost time (specs/002). A bare word becomes a
        // string in PowerShell, and `-ErrorAction SilentlyContinue` keeps
        // "process does not exist" from becoming an error.
        concat!(
            "if (Get-Process -Name oxdesk-host -ErrorAction SilentlyContinue) ",
            "{ Write-Output vivo } else { schtasks /run /tn oxdesk-host | Out-Null; ",
            "Start-Sleep -Seconds 2; ",
            "if (Get-Process -Name oxdesk-host -ErrorAction SilentlyContinue) ",
            "{ Write-Output vivo } else { Write-Output morto } }"
        )
        .to_string()
    } else if h.os.as_deref() == Some("macos") {
        // macOS: LaunchAgent, not `nohup`.
        //
        // Screen capture depends on a TCC permission (Screen Recording),
        // granted to whoever LAUNCHED the process. Coming from SSH there
        // is no grant at all -- and AVFoundation, instead of failing,
        // **hangs**. Measured: `ffmpeg -f avfoundation -i 1` itself hangs
        // forever over SSH.
        //
        // Same boundary that forces Windows to use a scheduled task with
        // /IT instead of a service.
        concat!(
            r#"pgrep -x oxdesk-host >/dev/null 2>&1 || "#,
            r#"launchctl kickstart "gui/$(id -u)/app.oxdesk.host" >/dev/null 2>&1; "#,
            r#"sleep 1; pgrep -x oxdesk-host >/dev/null && echo vivo || echo morto"#
        )
        .to_string()
    } else {
        format!(
            concat!(
                r#"pgrep -x oxdesk-host >/dev/null 2>&1 || "#,
                // `setsid` when it exists, `nohup` alone when it does not.
                //
                // macOS does NOT have setsid, and the whole line died with
                // "command not found" -- never reaching nohup. On Linux
                // setsid still matters: it takes the process out of the
                // SSH session, and without it the process goes down when
                // the connection closes.
                // The previous session's log is kept as `.1`: a session that
                // failed is usually diagnosed after the next one started,
                // and overwriting it lost the one case that mattered
                // (2026-11-07, a session with zero bytes and no log).
                r#"(mv -f "$HOME/.local/state/oxdesk-host.log" "$HOME/.local/state/oxdesk-host.log.1" 2>/dev/null; "#,
                r#"S=; command -v setsid >/dev/null 2>&1 && S=setsid; "#,
                r#"$S nohup "$HOME/.local/bin/oxdesk-host" {} "#,
                r#">"$HOME/.local/state/oxdesk-host.log" 2>&1 &); "#,
                r#"sleep 1; pgrep -x oxdesk-host >/dev/null && echo vivo || echo morto"#
            ),
            port
        )
    };
    let output = crate::remote::run(h, &cmd, 12)?;
    if output.contains("vivo") {
        say!("screen: screen host up on {}", h.name);
        return Ok(());
    }
    // The hint about where to look changes with the OS, and giving the
    // wrong one sends the person looking for a file that does not exist.
    Err(if crate::remote::is_powershell(h) {
        format!(
            "o host de tela nao subiu em {}: confira `schtasks /query /tn oxdesk-host` \
             la -- a tarefa precisa existir e a sessao estar aberta (ela roda com /it)",
            h.name
        )
    } else {
        format!(
            "o processo nao subiu em {} (veja ~/.local/state/oxdesk-host.log la)",
            h.name
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_template_vanishes_when_there_is_no_user() {
        // An empty `/u:` makes freerdp prompt for a user and hang without
        // a terminal -- the argument has to disappear, not stay empty.
        let args = vec!["/v:{host}:{port}", "{u_flag}", "+clipboard"];
        assert_eq!(
            render(&args, "pc.local", 3389, None),
            vec!["/v:pc.local:3389", "+clipboard"]
        );
        assert_eq!(
            render(&args, "pc.local", 3389, Some("alice")),
            vec!["/v:pc.local:3389", "/u:alice", "+clipboard"]
        );
    }

    #[test]
    fn uri_gets_the_user_attached() {

        let args = vec!["rdp://{user_at}{host}:{port}"];
        assert_eq!(render(&args, "pc.local", 3389, Some("v")), vec!["rdp://v@pc.local:3389"]);
        assert_eq!(render(&args, "pc.local", 3389, None), vec!["rdp://pc.local:3389"]);
    }
}
