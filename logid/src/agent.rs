//! Ponte para o agente do outro lado.
//!
//! Invocado por SSH de proposito: reaproveita a chave que ja temos, sem
//! porta nova nem auth nova. Ver ../specs/002-ssh.md.

use serde::Serialize;
use std::process::Command;

/// Caminho default do agente no Windows.
pub const DEFAULT_WIN_PATH: &str = r"%LOCALAPPDATA%\logid\logid-agent.exe";

#[derive(Debug, Serialize)]
pub struct Slot {
    pub slot: u8,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_hosts: Option<u8>,
    /// 1-based, como a config usa.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_host: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub reachable: bool,
    pub slots: Vec<Slot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Roda `logid-agent devices` no host remoto e interpreta a saida.
pub fn devices(ssh_alias: &str, exe: &str) -> AgentInfo {
    // PowerShell expande %VAR% so via $env:; montamos o comando la.
    let ps = format!("& \"{}\" devices", exe.replace("%LOCALAPPDATA%", "$env:LOCALAPPDATA"));
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=4",
            "-o", "LogLevel=ERROR",
            ssh_alias, &ps,
        ])
        .output();

    let Ok(out) = out else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("nao consegui executar ssh".into()),
        };
    };

    // O OpenSSH do Windows e antigo e imprime aviso de post-quantum em
    // stderr; a linha de JSON e a que importa.
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().find(|l| l.trim_start().starts_with('{')) else {
        let err = String::from_utf8_lossy(&out.stderr)
            .lines()
            .filter(|l| !l.contains("post-quantum") && !l.contains("openssh.com") && !l.contains("store now"))
            .last()
            .unwrap_or("agente nao respondeu JSON")
            .to_string();
        return AgentInfo { reachable: false, slots: vec![], error: Some(err) };
    };

    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("JSON do agente ilegivel".into()),
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
                    online: s.get("online").and_then(|x| x.as_bool()).unwrap_or(false),
                    num_hosts: s.get("num_hosts").and_then(|x| x.as_u64()).map(|v| v as u8),
                    current_host: s.get("current_host").and_then(|x| x.as_u64()).map(|v| v as u8),
                })
                .collect()
        })
        .unwrap_or_default();

    AgentInfo { reachable: true, slots, error: None }
}
