//! Session telemetry, logid's half: on/off, which path a session takes, and
//! reading back what was recorded.
//!
//! Spec: ../../specs/017-product.md (section 2)
//!
//! The bridge writes one line per session (`logistream::telemetry`); logid
//! decides whether it may (`enabled`) and tells it the path, which only this
//! side can find out: it knows the host's address and can ask Tailscale.

use std::net::IpAddr;
use std::path::PathBuf;

use crate::config::{Config, HostCfg};

pub fn enabled(cfg: &Config) -> bool {
    // The environment wins over the config, so a single run can be kept out.
    cfg.telemetry && std::env::var("LOGI_TELEMETRY").map_or(true, |v| v != "0")
}

impl Config {
    /// What the bridge needs to record: the file, or nothing when off.
    pub fn telemetry_env(&self) -> Vec<(&'static str, PathBuf)> {
        if enabled(self) {
            vec![("LOGI_TELEMETRY_FILE", file())]
        } else {
            vec![]
        }
    }
}

pub fn file() -> PathBuf {
    crate::sys::state_dir().join("telemetry.jsonl")
}

/// How this machine reaches `h` right now.
pub fn path_to(h: &HostCfg) -> String {
    let Some(ip) = resolve(&h.addr) else { return "unknown".into() };
    match classify(ip) {
        Class::Overlay => match tailscale_peer_direct(ip) {
            Some(Direct::Lan) => "overlay-lan".into(),
            Some(Direct::Internet) => "overlay-direct".into(),
            Some(Direct::Relay) => "overlay-relay".into(),
            None => "overlay".into(),
        },
        Class::Lan => "lan".into(),
        Class::Internet => "internet".into(),
    }
}

fn resolve(addr: &str) -> Option<IpAddr> {
    use std::net::ToSocketAddrs;
    let mut v: Vec<IpAddr> = (addr, 0).to_socket_addrs().ok()?.map(|s| s.ip()).collect();
    // Same preference as the bridge, which is the one that connects.
    v.sort_by_key(|a| !a.is_ipv4());
    v.first().copied()
}

#[derive(Debug, PartialEq)]
enum Class {
    Overlay,
    Lan,
    Internet,
}

fn classify(ip: IpAddr) -> Class {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // Tailscale uses the CGNAT range (RFC 6598). A phone hotspot can
            // hand out the same range, but the address here comes from the
            // configured host name, which is the tailnet name when the
            // machine is away (spec 012).
            if o[0] == 100 && (o[1] & 0xc0) == 64 {
                Class::Overlay
            } else if v4.is_private() || v4.is_link_local() || v4.is_loopback() {
                Class::Lan
            } else {
                Class::Internet
            }
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 {
                Class::Overlay
            } else if (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || v6.is_loopback() {
                Class::Lan
            } else {
                Class::Internet
            }
        }
    }
}

/// The Tailscale CLI, never the macOS app's shim in /usr/local/bin: calling
/// that one opens the app, which asks to enable the VPN that took this Mac's
/// internet down on 2026-11-07.
fn tailscale_bin() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/opt/homebrew/bin/tailscale"]
    } else if cfg!(windows) {
        &[r"C:\Program Files\Tailscale\tailscale.exe"]
    } else {
        &["/usr/bin/tailscale", "/usr/sbin/tailscale", "/usr/local/bin/tailscale"]
    };
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}

/// How Tailscale reaches a peer. At home the overlay still goes peer to
/// peer, but over the LAN: telling that apart from "direct across the
/// internet" is the whole point of measuring (home vs away).
#[derive(Debug, PartialEq)]
enum Direct {
    Lan,
    Internet,
    Relay,
}

/// `None` if it cannot tell.
fn tailscale_peer_direct(ip: IpAddr) -> Option<Direct> {
    let bin = tailscale_bin()?;
    let out = crate::sys::child(&bin.to_string_lossy())
        .args(["status", "--json"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    peer_direct(&String::from_utf8_lossy(&out.stdout), ip)
}

fn peer_direct(status_json: &str, ip: IpAddr) -> Option<Direct> {
    let v: serde_json::Value = serde_json::from_str(status_json).ok()?;
    let want = ip.to_string();
    v.get("Peer")?.as_object()?.values().find_map(|p| {
        let ips = p.get("TailscaleIPs")?.as_array()?;
        if !ips.iter().any(|x| x.as_str() == Some(want.as_str())) {
            return None;
        }
        // `CurAddr` is the direct endpoint in use ("ip:port"); empty means DERP.
        let cur = p.get("CurAddr").and_then(|c| c.as_str()).unwrap_or("");
        if cur.is_empty() {
            return Some(Direct::Relay);
        }
        let host = cur.rsplit_once(':').map_or(cur, |(h, _)| h).trim_matches(['[', ']']);
        // A link-local IPv6 carries its zone (`fe80::1%en0`), which IpAddr
        // does not parse.
        let host = host.split('%').next().unwrap_or(host);
        Some(match host.parse::<IpAddr>().map(classify) {
            Ok(Class::Lan) => Direct::Lan,
            _ => Direct::Internet,
        })
    })
}

/// The last `n` events and a count per path, for `/api/telemetry`.
pub fn summary_json(n: usize) -> String {
    let text = std::fs::read_to_string(file()).unwrap_or_default();
    let events: Vec<serde_json::Value> =
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let mut by_path: std::collections::BTreeMap<String, u64> = Default::default();
    for e in &events {
        let p = e.get("path").and_then(|p| p.as_str()).unwrap_or("unknown").to_string();
        *by_path.entry(p).or_default() += 1;
    }
    let last: Vec<&serde_json::Value> = events.iter().rev().take(n).collect();
    serde_json::json!({
        "file": file().display().to_string(),
        "sessions": events.len(),
        "by_path": by_path,
        "last": last,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_overlay_lan_and_internet() {
        let c = |s: &str| classify(s.parse().unwrap());
        assert_eq!(c("100.90.12.34"), Class::Overlay);
        assert_eq!(c("100.128.0.1"), Class::Internet, "just outside 100.64/10");
        assert_eq!(c("fd7a:115c:a1e0::1"), Class::Overlay);
        assert_eq!(c("192.168.1.20"), Class::Lan);
        assert_eq!(c("10.0.0.5"), Class::Lan);
        assert_eq!(c("fe80::1"), Class::Lan);
        assert_eq!(c("fd00::1"), Class::Lan, "a ULA that is not Tailscale's");
        assert_eq!(c("177.11.24.9"), Class::Internet);
    }

    #[test]
    fn reads_direct_or_relay_from_tailscale_status() {
        let j = r#"{"Peer":{
            "a":{"TailscaleIPs":["100.1.1.1","fd7a:115c:a1e0::1"],"CurAddr":"177.11.24.9:41641","Relay":"gru"},
            "b":{"TailscaleIPs":["100.2.2.2"],"CurAddr":"","Relay":"gru"},
            "c":{"TailscaleIPs":["100.4.4.4"],"CurAddr":"192.168.0.10:41641"},
            "d":{"TailscaleIPs":["100.5.5.5"],"CurAddr":"[2804:14c::1]:41641"},
            "e":{"TailscaleIPs":["100.6.6.6"],"CurAddr":"[fe80::1%en0]:41641"}}}"#;
        let at = |s: &str| peer_direct(j, s.parse().unwrap());
        assert_eq!(at("100.1.1.1"), Some(Direct::Internet));
        assert_eq!(at("fd7a:115c:a1e0::1"), Some(Direct::Internet));
        assert_eq!(at("100.2.2.2"), Some(Direct::Relay));
        assert_eq!(at("100.4.4.4"), Some(Direct::Lan));
        assert_eq!(at("100.5.5.5"), Some(Direct::Internet));
        assert_eq!(at("100.6.6.6"), Some(Direct::Lan), "link-local with a zone");
        assert_eq!(peer_direct(j, "100.3.3.3".parse().unwrap()), None);
        assert_eq!(peer_direct("not json", "100.1.1.1".parse().unwrap()), None);
    }
}
