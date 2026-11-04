//! Implementacao Linux do contrato em `mod.rs`.

use crate::config::Config;
use crate::logs::Raw;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::Command;
use std::sync::{Arc, RwLock};

const UNIT: &str = "logid.service";

/// Resolve por `getent` quando o resolver da libc desiste.
///
/// `to_socket_addrs` usa getaddrinfo com AF_UNSPEC, e isso **falha** para
/// `.local` nesta maquina: `mdns4_minimal` com `[NOTFOUND=return]` no
/// nsswitch aborta a cadeia na consulta AAAA. Comprovado: `ssh-keyscan
/// windows-host.local` da "Temporary failure in name resolution" enquanto
/// `getent hosts` funciona.
pub fn resolve_fallback(addr: &str, port: u16) -> Option<SocketAddr> {
    let out = Command::new("getent").arg("hosts").arg(addr).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let ip = text.split_whitespace().next()?;
    (ip, port).to_socket_addrs().ok()?.next()
}

pub fn unit_state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

/// Hora local HH:MM:SS, sem puxar uma crate de data so para isso.
pub fn hhmmss() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let t = (secs + offset_secs()).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

fn offset_secs() -> i64 {
    let Ok(out) = Command::new("date").arg("+%z").output() else { return 0 };
    let z = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if z.len() < 5 {
        return 0;
    }
    let sign = if z.starts_with('-') { -1 } else { 1 };
    let h: i64 = z[1..3].parse().unwrap_or(0);
    let m: i64 = z[3..5].parse().unwrap_or(0);
    sign * (h * 3600 + m * 60)
}

/// Aqui o journal timbra e persiste; o daemon so escreve em stdout.
pub fn log_sink(_line: &str) {}

pub fn log_lines(n: usize) -> Raw {
    let out = Command::new("journalctl")
        .args(["--user", "-u", UNIT, "-n", &n.to_string(), "--no-pager", "-o", "short-iso"])
        .output();

    let Ok(out) = out else {
        return Raw {
            unit: UNIT.into(),
            lines: vec![],
            error: Some("nao consegui executar journalctl".into()),
        };
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.starts_with("--") {
            continue; // cabecalho do journal
        }
        // "2026-11-03T18:51:18-0300 host logid[123]: mensagem"
        let Some((time, rest)) = line.split_once(' ') else { continue };
        let msg = match rest.split_once("]: ") {
            Some((_, m)) => m,
            None => rest.split_once(": ").map(|(_, m)| m).unwrap_or(rest),
        };
        lines.push((time.get(11..19).unwrap_or(&time).to_string(), msg.to_string()));
    }
    Raw { unit: UNIT.into(), lines, error: None }
}

pub fn spawn_border(cfg: Arc<RwLock<Config>>) {
    crate::border::spawn(cfg);
}

/// Conseguimos abrir algum `/dev/hidraw`?
///
/// A regra do Solaar concede por `TAG+="uaccess"`, que vira ACL para o
/// usuario logado. Pegadinha paga: se os nos nasceram ANTES de a regra
/// existir, nenhuma ACL foi aplicada e so `udevadm trigger` resolve.
pub fn device_access() -> bool {
    let Ok(entries) = std::fs::read_dir("/dev") else { return false };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if n.starts_with("hidraw") && std::fs::File::open(e.path()).is_ok() {
            return true;
        }
    }
    false
}

/// Onde mora a config: `$XDG_CONFIG_HOME/logi` ou `~/.config/logi`.
pub fn config_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("logi")
}
