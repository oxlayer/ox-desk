//! Sondagem: portas TCP, units systemd e detalhe por SSH.

use crate::config::{Config, Expect, HostCfg};
use serde::Serialize;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};

const TCP_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Serialize)]
pub struct ServiceState {
    pub label: String,
    pub port: u16,
    pub up: bool,
    pub ms: Option<f64>,
    /// Esta como deveria para ESTE papel (ver Expect).
    pub ok: bool,
    pub expect: Expect,
}

#[derive(Debug, Serialize)]
pub struct UnitState {
    pub unit: String,
    pub state: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Detail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_min: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_free_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram_total_gb: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct HostState {
    pub name: String,
    pub addr: String,
    pub role: String,
    pub online: bool,
    pub services: Vec<ServiceState>,
    pub units: Vec<UnitState>,
    pub detail: Detail,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub generated: String,
    pub hosts: Vec<HostState>,
}

/// Resolve nome -> SocketAddr.
///
/// `to_socket_addrs` usa getaddrinfo com AF_UNSPEC, e isso **falha** para
/// `.local` nesta maquina: `mdns4_minimal` com `[NOTFOUND=return]` no
/// nsswitch aborta a cadeia na consulta AAAA. Comprovado: `ssh-keyscan
/// windows-host.local` da "Temporary failure in name resolution" enquanto
/// `getent hosts` funciona. Dai o fallback por getent.
fn resolve(addr: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(mut it) = (addr, port).to_socket_addrs() {
        if let Some(sa) = it.next() {
            return Some(sa);
        }
    }
    let out = Command::new("getent").arg("hosts").arg(addr).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let ip = text.split_whitespace().next()?;
    (ip, port).to_socket_addrs().ok()?.next()
}

/// (aberta, latencia_ms)
pub fn tcp_probe(addr: &str, port: u16) -> (bool, Option<f64>) {
    let Some(sa) = resolve(addr, port) else {
        return (false, None);
    };
    let t0 = Instant::now();
    match TcpStream::connect_timeout(&sa, TCP_TIMEOUT) {
        Ok(_) => (true, Some(t0.elapsed().as_secs_f64() * 1000.0)),
        Err(_) => (false, None),
    }
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

/// Uptime e RAM do Windows por SSH. Silencioso em qualquer falha: detalhe
/// e enfeite, nao pode derrubar o snapshot.
pub fn ssh_detail(alias: &str) -> Detail {
    let ps = concat!(
        "$o=Get-CimInstance Win32_OperatingSystem;",
        "$c=Get-CimInstance Win32_ComputerSystem;",
        "$up=[int]((Get-Date)-$o.LastBootUpTime).TotalMinutes;",
        "$free=[math]::Round($o.FreePhysicalMemory/1MB,1);",
        "$tot=[math]::Round($c.TotalPhysicalMemory/1GB,1);",
        "\"$up|$free|$tot\""
    );
    let out = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=3",
            "-o", "LogLevel=ERROR",
            alias, ps,
        ])
        .output();
    let Ok(out) = out else { return Detail::default() };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().filter(|l| l.contains('|')).next_back() else {
        return Detail::default();
    };
    let parts: Vec<&str> = line.trim().split('|').collect();
    if parts.len() != 3 {
        return Detail::default();
    }
    Detail {
        uptime_min: parts[0].trim().parse::<f64>().ok().map(|v| v as u64),
        ram_free_gb: parts[1].trim().parse().ok(),
        ram_total_gb: parts[2].trim().parse().ok(),
    }
}

fn check_host(h: &HostCfg) -> HostState {
    let services: Vec<ServiceState> = std::thread::scope(|s| {
        let handles: Vec<_> = h
            .services
            .iter()
            .map(|svc| s.spawn(|| {
                let (up, ms) = tcp_probe(&h.addr, svc.port);
                let ok = match svc.expect {
                    Expect::Any => true,
                    Expect::Down => !up,
                    Expect::Up => up,
                };
                ServiceState {
                    label: svc.label.clone(),
                    port: svc.port,
                    up,
                    ms: ms.map(|v| (v * 10.0).round() / 10.0),
                    ok,
                    expect: svc.expect,
                }
            }))
            .collect();
        let mut v: Vec<ServiceState> = handles.into_iter().filter_map(|x| x.join().ok()).collect();
        v.sort_by_key(|s| s.port);
        v
    });

    let online = services.iter().any(|s| s.up);
    let units = h
        .units
        .iter()
        .map(|u| UnitState { unit: u.clone(), state: unit_state(u) })
        .collect();
    let detail = match (&h.ssh, online) {
        (Some(alias), true) => ssh_detail(alias),
        _ => Detail::default(),
    };

    HostState {
        name: h.name.clone(),
        addr: h.addr.clone(),
        role: h.role.clone(),
        online,
        services,
        units,
        detail,
    }
}

fn hhmmss() -> String {
    // Sem crate de data: segundos desde a epoca -> HH:MM:SS local.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let off = local_offset_secs();
    let t = (secs + off).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}

/// Offset local em segundos, via `date +%z` (evita puxar chrono so para isso).
fn local_offset_secs() -> i64 {
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

pub fn snapshot(cfg: &Config) -> Snapshot {
    let hosts = std::thread::scope(|s| {
        let handles: Vec<_> = cfg.hosts.iter().map(|h| s.spawn(|| check_host(h))).collect();
        handles.into_iter().filter_map(|x| x.join().ok()).collect()
    });
    Snapshot { generated: hhmmss(), hosts }
}
