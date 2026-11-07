//! Run a shell line on a host -- here or on the peer.
//!
//! Extracted the third time it showed up. `probe::host_detail` and `agent`
//! already built `ssh` with the same four options plus the resolution
//! workaround, and the process manager would need the same. The fourth
//! copy would be where one of them forgot `ssh_addr_opts` and failed only
//! on Windows -- which is exactly how that bug showed up the first time.
//!
//! The caller picks the command according to the TARGET's OS
//! (`HostCfg::os`), not ours: running PowerShell against Ubuntu fails
//! silently.

use crate::config::{HostCfg, HostOs};

/// Runs `cmd` on the host and returns stdout.
///
/// Local: through this machine's shell. Remote: over SSH, with the
/// address already resolved by us (see `probe::ssh_addr_opts`).
pub fn run(h: &HostCfg, cmd: &str, timeout_s: u32) -> Result<String, String> {
    let out = if h.is_local() {
        let (prog, flags): (&str, &[&str]) = if cfg!(windows) {
            ("powershell", &["-NoProfile", "-Command"])
        } else {
            ("sh", &["-c"])
        };
        crate::sys::child(prog)
            .args(flags)
            .arg(cmd)
            .output()
            .map_err(|e| format!("executar local: {e}"))?
    } else {
        let alias = h.ssh.as_deref().ok_or_else(|| {
            format!("host '{}' nao tem `ssh` na config: nao ha como rodar nada la", h.name)
        })?;
        let mut args: Vec<String> = vec![
            "-o".into(), "BatchMode=yes".into(),
            "-o".into(), format!("ConnectTimeout={timeout_s}"),
            "-o".into(), "LogLevel=ERROR".into(),
        ];
        args.extend(crate::probe::ssh_addr_opts(&h.addr));
        args.push(alias.to_string());
        args.push(cmd.to_string());
        crate::sys::child("ssh").args(&args).output().map_err(|e| format!("ssh: {e}"))?
    };

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        // Windows OpenSSH prints a post-quantum warning on stderr; the
        // useful message is the last line that is not that.
        let err = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| {
                !l.contains("post-quantum") && !l.contains("openssh.com") && !l.contains("store now")
            })
            .filter(|l| !l.trim().is_empty())
            .last()
            .unwrap_or("comando falhou sem mensagem")
            .to_string();
        return Err(err);
    }
    Ok(stdout)
}

/// Does the host's shell speak PowerShell?
pub fn is_powershell(h: &HostCfg) -> bool {
    h.os() == HostOs::Windows
}

/// Command line to invoke OUR `logid` on the peer, over SSH.
///
/// It looks like a detail and is not: on Windows `logid.exe` is
/// `windows_subsystem = "windows"` (no console, so the daemon does not
/// flash a window), and **PowerShell does not wait for a GUI
/// executable**. Without waiting, two kinds of damage:
///
/// 1. `$LASTEXITCODE` is never set, so SSH returns 0 and the origin
///    announces "received" with the other side empty.
/// 2. The SSH session closes while the process is still reading stdin,
///    and what is left is a race.
///
/// Measured: three text sends without redirecting the output arrived
/// **twice**; with redirection, three out of three. A small payload
/// usually wins the race, which is why the text clipboard seemed to work
/// -- a `tar` is bigger and loses more.
///
/// `| Out-String | Write-Output` forces PowerShell to consume the
/// process's output, and consuming implies waiting for the end.
/// `; exit $LASTEXITCODE` hands the real code back to SSH.
pub fn logid_command(h: &HostCfg, sub: &str) -> String {
    if is_powershell(h) {
        format!(
            r#"& "$env:LOCALAPPDATA\logid\logid.exe" {sub} | Out-String | Write-Output; exit $LASTEXITCODE"#
        )
    } else {
        // `~` expands in the remote shell; an absolute path would require
        // knowing each machine's user name.

        format!("~/.local/bin/logid {sub}")
    }
}
