//! Ponte para o agente do outro lado.
//!
//! Invocado por SSH de proposito: reaproveita a chave que ja temos, sem
//! porta nova nem auth nova. Ver ../specs/002-ssh.md.

use crate::config::{HostCfg, HostOs};
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct Slot {
    pub slot: u8,
    /// Nome HID++, quando o agente consegue ler. E por ele que o
    /// reconciliador casa dispositivo: canal e ambiguo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_hosts: Option<u8>,
    /// 1-based, como a config usa.
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

/// Monta a linha a executar NO par, respeitando o shell de lá.
///
/// No Windows o shell e o PowerShell, que expande `%VAR%` so via
/// `$env:`, e precisa do operador `&` para executar um caminho entre
/// aspas. No Unix e `sh`: aspas simples bastam, e `$HOME` ja e expandido
/// pelo proprio shell.
///
/// Rodar a forma errada falha de um jeito chato: no Unix o `& "..."`
/// viraria "comando em background" e devolveria sucesso vazio, que o
/// chamador leria como "agente nao respondeu JSON".
fn remote_cmd(os: HostOs, exe: &str, args: &str) -> String {
    match os {
        HostOs::Windows => {
            format!("& \"{}\" {args}", exe.replace("%LOCALAPPDATA%", "$env:LOCALAPPDATA"))
        }
        // `Unknown` tratado como Unix: e o default seguro depois de o par
        // deixar de ser sempre Windows, e um caminho com `%` ou `\` ja foi
        // classificado como Windows por `HostCfg::os`.
        _ => format!("'{}' {args}", exe.replace('\'', "")),
    }
}

/// Roda `<agent> devices` no host remoto e interpreta a saida.
///
/// Recebe o host inteiro, e nao alias+exe+os soltos, porque precisa de
/// quatro coisas dele: o alias, o caminho do agente, o SO (para montar o
/// comando no shell certo) e o `addr` (para resolver aqui em vez de
/// deixar o ssh resolver -- ver `probe::ssh_addr_opts`).
pub fn devices(h: &HostCfg) -> AgentInfo {
    let (Some(ssh_alias), Some(exe)) = (&h.ssh, &h.agent) else {
        return AgentInfo {
            reachable: false,
            slots: vec![],
            error: Some("host sem `ssh` e/ou `agent` na config".into()),
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
    let out = Command::new("ssh").args(&args).output();

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

/// Pede ao agente do par que mande um slot para outro canal.
///
/// Usado pelo reconciliador: quem detem o dispositivo e quem pode
/// empurra-lo, e as vezes nao somos nos.
pub fn set_host(h: &HostCfg, slot: u8, host: u8) -> Result<(), String> {
    let (Some(ssh_alias), Some(exe)) = (&h.ssh, &h.agent) else {
        return Err("host sem `ssh` e/ou `agent` na config".into());
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
    let out = Command::new("ssh").args(&args).output().map_err(|e| format!("ssh: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or("agente nao respondeu JSON")?;
    let v: serde_json::Value =
        serde_json::from_str(line).map_err(|_| "JSON do agente ilegivel".to_string())?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("falhou").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comando_remoto_respeita_o_shell_do_par() {
        // Windows: PowerShell precisa do `&` e expande %VAR% via $env:.
        assert_eq!(
            remote_cmd(HostOs::Windows, r"%LOCALAPPDATA%\logid\logid-agent.exe", "devices"),
            "& \"$env:LOCALAPPDATA\\logid\\logid-agent.exe\" devices"
        );
        // Unix: aspas simples, sem `&` -- que ali significaria background
        // e devolveria sucesso vazio, lido como "nao respondeu JSON".
        assert_eq!(
            remote_cmd(HostOs::Linux, "/home/user/.local/bin/logid", "devices"),
            "'/home/user/.local/bin/logid' devices"
        );
        assert_eq!(
            remote_cmd(HostOs::Macos, "/opt/homebrew/bin/logid", "set-host 1 2 --confirm"),
            "'/opt/homebrew/bin/logid' set-host 1 2 --confirm"
        );
        // Desconhecido cai no Unix, que e o default seguro agora.
        assert_eq!(
            remote_cmd(HostOs::Unknown, "/usr/local/bin/logid", "devices"),
            "'/usr/local/bin/logid' devices"
        );
    }
}
