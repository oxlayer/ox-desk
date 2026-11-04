//! Sondagem: portas TCP, estado dos `units` e detalhe por SSH.
//!
//! O que `units` significa muda por plataforma -- unit do systemd no
//! Linux, servico ou processo no Windows. Ver `sys::unit_state`.

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

/// Resolve nome -> SocketAddr, com o fallback que a plataforma precisar.
///
/// O caminho padrao e `to_socket_addrs`. Quando ele desiste, `sys` decide
/// o que fazer -- no Linux ha um buraco real de mDNS a tapar, no Windows
/// nao ha. Ver `sys::resolve_fallback`.
fn resolve(addr: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(mut it) = (addr, port).to_socket_addrs() {
        if let Some(sa) = it.next() {
            return Some(sa);
        }
    }
    crate::sys::resolve_fallback(addr, port)
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
    // Units SO da maquina local. `unit_state` consulta o gerenciador
    // daqui: fazer isso por um host remoto devolveria o estado da maquina
    // errada com cara de verdade. Hoje nenhuma config remota lista units,
    // mas a primeira que listasse mentiria sem avisar.
    let units = if h.is_local() {
        h.units
            .iter()
            .map(|u| UnitState { unit: u.clone(), state: crate::sys::unit_state(u) })
            .collect()
    } else {
        h.units
            .iter()
            .map(|u| UnitState { unit: u.clone(), state: "remoto".into() })
            .collect()
    };
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

pub fn snapshot(cfg: &Config) -> Snapshot {
    let hosts = std::thread::scope(|s| {
        let handles: Vec<_> = cfg.hosts.iter().map(|h| s.spawn(|| check_host(h))).collect();
        handles.into_iter().filter_map(|x| x.join().ok()).collect()
    });
    Snapshot { generated: crate::sys::hhmmss(), hosts }
}
