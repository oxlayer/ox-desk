// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bridge to the agent on the other side.
//!
//! Invoked over SSH on purpose: it reuses the key we already have, with
//! no new port and no new auth. See ../specs/002-ssh.md.

use crate::config::{HostCfg, HostOs};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Slot {
    pub slot: u8,
    /// HID++ name, when the agent can read it. The reconciler matches
    /// devices by it: the channel is ambiguous.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_hosts: Option<u8>,
    /// 1-based, as the config uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_host: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub reachable: bool,
    pub slots: Vec<Slot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Builds the line to run ON the peer, honoring the shell there.
///
/// On Windows the shell is PowerShell, which expands `%VAR%` only via
/// `$env:`, and needs the `&` operator to run a quoted path. On Unix it
/// is `sh`: single quotes are enough, and `$HOME` is expanded by the
/// shell itself.
///
/// Running the wrong form fails in an annoying way: on Unix `& "..."`
/// would become "background command" and return empty success, which
/// the caller would read as "agent did not answer JSON".
fn remote_cmd(os: HostOs, exe: &str, args: &str) -> String {
    match os {
        HostOs::Windows => {
            format!("& \"{}\" {args}", exe.replace("%LOCALAPPDATA%", "$env:LOCALAPPDATA"))
        }
        // `Unknown` treated as Unix: it is the safe default now that the
        // peer is no longer always Windows, and a path with `%` or `\` has
        // already been classified as Windows by `HostCfg::os`.
        _ => format!("'{}' {args}", exe.replace('\'', "")),
    }
}

/// Runs `<agent> devices` on the remote host and parses the output.
///
/// Takes the whole host, not loose alias+exe+os, because it needs four
/// things from it: the alias, the agent path, the OS (to build the
/// command for the right shell) and the `addr` (to resolve here instead
/// of letting ssh resolve -- see `probe::ssh_addr_opts`).
pub fn devices(h: &HostCfg) -> AgentInfo {
    let (Some(ssh_alias), Some(exe)) = (&h.ssh, &h.agent) else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("host has no `ssh` and/or `agent` in the config".into()),
        };
    };
    let ps = remote_cmd(h.os(), exe, "devices");
    let mut args: Vec<String> = vec![
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "ConnectTimeout=4".into(),
        "-o".into(), "LogLevel=ERROR".into(),
    ];
    args.extend(crate::probe::ssh_addr_opts(&h.addr));
    args.push(ssh_alias.clone());
    args.push(ps);
    let out = crate::sys::child("ssh").args(&args).output();

    let Ok(out) = out else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("could not run ssh".into()),
        };
    };

    // Windows' OpenSSH is old and prints a post-quantum warning on
    // stderr; the JSON line is the one that matters.
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().find(|l| l.trim_start().starts_with('{')) else {
        let err = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| !l.contains("post-quantum") && !l.contains("openssh.com") && !l.contains("store now"))
            .last()
            .unwrap_or("agent did not answer JSON")
            .to_string();
        return AgentInfo { reachable: false, slots: vec![], error: Some(err) };
    };

    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("unreadable agent JSON".into()),
        };
    };
    if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: v.get("error").and_then(|e| e.as_str()).map(String::from),
        };
    }

    let slots = v
        .get("slots")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .map(|s| Slot {
                    slot: s.get("slot").and_then(|x| x.as_u64()).unwrap_or(0) as u8,
                    name: s.get("name").and_then(|x| x.as_str()).map(String::from),
                    online: s.get("online").and_then(|x| x.as_bool()).unwrap_or(false),
                    num_hosts: s.get("num_hosts").and_then(|x| x.as_u64()).map(|v| v as u8),
                    current_host: s.get("current_host").and_then(|x| x.as_u64()).map(|v| v as u8),
                })
                .collect()
        })
        .unwrap_or_default();

    AgentInfo { reachable: true, slots, error: None }
}

/// Asks the peer's agent to send a slot to another channel.
///
/// Used by the reconciler: whoever holds the device is the one who can
/// push it, and sometimes that is not us.
pub fn set_host(h: &HostCfg, slot: u8, host: u8) -> Result<(), String> {
    let (Some(ssh_alias), Some(exe)) = (&h.ssh, &h.agent) else {
        return Err("host has no `ssh` and/or `agent` in the config".into());
    };
    let ps = remote_cmd(h.os(), exe, &format!("set-host {slot} {host} --confirm"));
    let mut args: Vec<String> = vec![
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "ConnectTimeout=4".into(),
        "-o".into(), "LogLevel=ERROR".into(),
    ];
    args.extend(crate::probe::ssh_addr_opts(&h.addr));
    args.push(ssh_alias.clone());
    args.push(ps);
    let out = crate::sys::child("ssh").args(&args).output().map_err(|e| format!("ssh: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or("agent did not answer JSON")?;
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|_| "unreadable agent JSON".to_string())?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("failed").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_command_honors_the_peer_shell() {
        // Windows: PowerShell needs the `&` and expands %VAR% via $env:.
        assert_eq!(
            remote_cmd(HostOs::Windows, r"%LOCALAPPDATA%\oxdesk\oxdesk-agent.exe", "devices"),
            "& \"$env:LOCALAPPDATA\\oxdesk\\oxdesk-agent.exe\" devices"
        );
        // Unix: single quotes, no `&` -- which there would mean background
        // and return empty success, read as "did not answer JSON".
        assert_eq!(
            remote_cmd(HostOs::Linux, "/home/alice/.local/bin/oxdesk", "devices"),
            "'/home/alice/.local/bin/oxdesk' devices"
        );
        assert_eq!(
            remote_cmd(HostOs::Macos, "/opt/homebrew/bin/oxdesk", "set-host 1 2 --confirm"),
            "'/opt/homebrew/bin/oxdesk' set-host 1 2 --confirm"
        );
        // Unknown falls back to Unix, which is the safe default now.
        assert_eq!(
            remote_cmd(HostOs::Unknown, "/usr/local/bin/oxdesk", "devices"),
            "'/usr/local/bin/oxdesk' devices"
        );
    }
}
