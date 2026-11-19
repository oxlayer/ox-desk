// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Probing: TCP ports, `units` state and details over SSH.
//!
//! What `units` means changes per platform -- a systemd unit on Linux, a
//! service or process on Windows. See `sys::unit_state`.

use crate::config::{Config, Expect, HostCfg, HostOs};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const TCP_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Serialize)]
pub struct ServiceState {
    pub label: String,
    pub port: u16,
    pub up: bool,
    pub ms: Option<f64>,
    /// Is as it should be for THIS role (see Expect).
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
    /// CPU usage, 0-100.
    ///
    /// The sampling WINDOW differs per OS, and that is deliberate: on
    /// Linux it is two reads of `/proc/stat` 200 ms apart (exact
    /// percentage), on macOS the sum of the processes' `%cpu` divided by
    /// the cores (an average that decays over ~1 min) and on Windows WMI's
    /// `LoadPercentage` (~1 s). None is comparable to the others to the
    /// decimal -- all of them answer "is this machine busy?", which is the
    /// question the card asks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    /// Space on the system drive, in GB: free (what this user can still
    /// write) and total. The system drive is `C:` (`$env:SystemDrive`) on
    /// Windows and `/` elsewhere. See `disk`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_free_gb: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_total_gb: Option<f64>,
    /// What those two mean (`disk::level`), decided here so the card, the
    /// summary and the log agree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_level: Option<crate::disk::Level>,
}

/// KVM state of **that** host: border and reconciler.
///
/// It exists because the panel only showed this machine's switches, and
/// the border is a PAIR behavior: the Windows one, pointed here, sent
/// everything back on every touch while this screen said, rightly, that
/// the border here was off. The user had no way to see that from
/// anywhere -- they had to open the other machine's panel.
#[derive(Debug, Default, Serialize)]
pub struct Kvm {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    /// The active edges there, like "left -> linux-pc".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<String>,
    /// Why we do not know. Better to say we do not know than to show
    /// "off" for a host that did not answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    pub kvm: Kvm,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub generated: String,
    pub hosts: Vec<HostState>,
}

/// All the addresses of a name, without link-local, IPv4 first.
///
/// Returns a LIST because a name can have families that do not behave
/// the same. Measured: seen from the Mac, `windows-pc.local` resolves to
/// IPv4 **and** IPv6, IPv4 connects on 3389 and IPv6 times out -- because
/// the Windows firewall rule was written as `192.168.1.0/24`, which is
/// IPv4. Taking only the first address, the Mac's panel said "RDP closed"
/// about a service that was open.
///
/// Link-local is dropped because it is useless without the right scope
/// (see `resolve_for_ssh`).
fn resolve_all(addr: &str, port: u16) -> Vec<SocketAddr> {
    fn link_local(a: &SocketAddr) -> bool {
        match a {
            SocketAddr::V4(v) => v.ip().octets()[..2] == [169, 254],
            SocketAddr::V6(v) => v.ip().segments()[0] & 0xffc0 == 0xfe80,
        }
    }
    let mut all: Vec<SocketAddr> =
        (addr, port).to_socket_addrs().map(|it| it.collect()).unwrap_or_default();
    if all.is_empty() {
        all = crate::sys::resolve_fallback(addr, port).into_iter().collect();
    }
    all.retain(|a| !link_local(a));
    // IPv4 first: on this LAN it is the path that always works, and trying
    // what works first saves the timeout of what does not.
    all.sort_by_key(|a| !a.is_ipv4());
    all.truncate(3);
    if let (Some(first), Ok(mut seen)) = (all.first(), LAST_RESOLVED.lock()) {
        seen.insert(addr.to_string(), (first.ip(), Instant::now()));
    }
    all
}

/// The last address each name resolved to, refreshed by every probe.
///
/// mDNS on Windows flaps: on the reference Windows machine the Ubuntu
/// machine's `.local` name stopped resolving for ~20 s every few minutes
/// (`probe: ...:22 stopped: the name did not resolve`, then `is back`). The
/// LAN KVM landed in one of those windows and died with "host not found"
/// (spec 024). Remembering the last answer is not writing an IP in the
/// config (spec 007): it lives in memory, is replaced at the next answer,
/// and the host's pinned identity still refuses a different machine at an
/// address that was reused.
static LAST_RESOLVED: Mutex<std::collections::BTreeMap<String, (std::net::IpAddr, Instant)>> =
    Mutex::new(std::collections::BTreeMap::new());

/// An answer this fresh is used without asking again. The probe refreshes
/// every name every ~10 s, so on a running daemon it is nearly always there,
/// and asking `.local` again on Windows is where a switch spent seconds
/// when mDNS was slow to answer.
const FRESH: Duration = Duration::from_secs(30);

/// Resolves a name: a fresh answer from the probe, else now, else the last
/// address it ever resolved to. The flag is `true` only for the last case.
pub fn resolve_with_memory(addr: &str, port: u16) -> Option<(SocketAddr, bool)> {
    let remembered = LAST_RESOLVED.lock().ok().and_then(|s| s.get(addr).copied());
    if let Some((ip, at)) = remembered {
        if at.elapsed() < FRESH {
            return Some((SocketAddr::new(ip, port), false));
        }
    }
    if let Some(sa) = resolve_all(addr, port).into_iter().next() {
        return Some((sa, false));
    }
    remembered.map(|(ip, _)| (SocketAddr::new(ip, port), true))
}

/// ssh/sftp options that bypass ssh's OWN resolver.
///
/// ## Why this exists
///
/// On Windows, `ssh` launched **by the daemon** fails with "Could not
/// resolve hostname linux-pc" -- while, in the same scheduled-task
/// context, `ping` resolves, `[System.Net.Dns]::GetHostAddresses`
/// resolves both names (short and `.local`), and the same `ssh` launched
/// from a `cmd.exe` in the same task works. That is: the OS resolver sees
/// the name; `ssh`'s, in that process, does not.
///
/// That would have hurt silently: the manual test passes and the daemon
/// fails. It is the same family as the mDNS hole on Linux, where
/// `getaddrinfo` with AF_UNSPEC gives up on `.local` and only `getent`
/// resolves (see `resolve`).
///
/// The way out is resolving **here**, where it works, and handing over
/// the ready address. It does not break the never-write-an-IP rule (spec
/// 007): the config keeps the name, resolution happens on every call, and
/// an IP change is absorbed. `HostKeyAlias` keeps the key's identity tied
/// to the NAME, not the address -- without it, every new IP would look
/// like an unknown host.
pub fn ssh_addr_opts(addr: &str) -> Vec<String> {
    // Port 22 only for resolution; we do not connect here.
    let Some(sa) = resolve_for_ssh(addr, 22) else { return Vec::new() };
    vec![
        "-o".into(),
        format!("HostName={}", sa.ip()),
        "-o".into(),
        format!("HostKeyAlias={addr}"),
    ]
}

/// Address to hand to `ssh`: the first from `resolve_all`.
///
/// Link-local is what made SFTP to the Mac die with "Connection closed":
/// seen from Windows, `the-mac.local` resolves **only to
/// IPv6**, and the first address is `fe80::...%11`, which without the
/// right scope leads nowhere. `None` means letting `ssh` try on its own
/// -- better than handing over an address that does not connect.
///
/// Through the probe's fresh answer when there is one: every ssh the daemon
/// starts used to resolve `.local` again, and on Windows that lookup alone
/// was most of the half second a switch spent waking the other host.
fn resolve_for_ssh(addr: &str, port: u16) -> Option<SocketAddr> {
    resolve_with_memory(addr, port).map(|(sa, _)| sa)
}

/// (open, latency_ms) -- tries each address until one connects.
pub fn tcp_probe(addr: &str, port: u16) -> (bool, Option<f64>) {
    let targets = resolve_all(addr, port);
    if targets.is_empty() {
        note_failure(addr, port, Some("the name did not resolve".into()));
        return (false, None);
    }
    // Splits the budget: with three dead addresses, a full timeout per
    // address would be 4.5 s for a single service in the snapshot.
    let per_target = TCP_TIMEOUT / targets.len() as u32;
    // Two rounds: over the tailnet a lost packet made the machine blink
    // offline on the panel, with ping and SSH up. A dead machine costs
    // twice as much.
    for _ in 0..2 {
        for sa in &targets {
            let t0 = Instant::now();
            if TcpStream::connect_timeout(sa, per_target).is_ok() {
                note_failure(addr, port, None);
                return (true, Some(t0.elapsed().as_secs_f64() * 1000.0));
            }
        }
    }
    note_failure(addr, port, Some(format!("no answer from {targets:?}")));
    (false, None)
}

/// Logs when a port stops answering and when it comes back, with the
/// reason.
///
/// Only on a state change, otherwise a machine that is off would fill the
/// log. It exists because the panel showed a live machine as offline and
/// nothing said why: there was no way to tell a name that did not resolve
/// from a connection with no answer.
fn note_failure(addr: &str, port: u16, reason: Option<String>) {
    static FAILING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let key = format!("{addr}:{port}");
    let Ok(mut g) = FAILING.lock() else { return };
    let set = g.get_or_insert_with(HashSet::new);
    match reason {
        Some(m) if set.insert(key.clone()) => say!("probe: {key} stopped: {m}"),
        None if set.remove(&key) => say!("probe: {key} is back"),
        _ => {}
    }
}

/// The peer's uptime and RAM over SSH. Silent on any failure: the detail
/// is decoration, it must not take the snapshot down.
///
/// The command is per OS. The first version always ran PowerShell with
/// CIM, because the peer was always Windows; against Ubuntu or macOS that
/// would fail silently and the card would have no detail forever, with
/// nothing saying why.
///
/// Output contract, the same on all three:
/// `<minutes>|<ram_free_gb>|<ram_total_gb>|<cpu_pct>|<disk_free_gb>|<disk_total_gb>`.
/// Fields are only ever added at the end, so a reader of a shorter line
/// keeps the ones it has (`host_detail`).
fn detail_cmd(os: HostOs) -> Option<&'static str> {
    Some(match os {
        HostOs::Windows => concat!(
            "$o=Get-CimInstance Win32_OperatingSystem;",
            "$c=Get-CimInstance Win32_ComputerSystem;",
            "$up=[int]((Get-Date)-$o.LastBootUpTime).TotalMinutes;",
            "$free=[math]::Round($o.FreePhysicalMemory/1MB,1);",
            "$tot=[math]::Round($c.TotalPhysicalMemory/1GB,1);",
            "$cpu=[int](Get-CimInstance Win32_Processor | ",
            "Measure-Object -Property LoadPercentage -Average).Average;",
            // The system drive by its own variable, not "C": it is C: on
            // nearly every machine, and the one where it is not is the one
            // a hardcoded letter would misreport.
            "$sd=Get-PSDrive -Name ($env:SystemDrive.Substring(0,1));",
            "$df=[math]::Round($sd.Free/1GB,2);",
            "$dt=[math]::Round(($sd.Used+$sd.Free)/1GB,1);",
            "\"$up|$free|$tot|$cpu|$df|$dt\""
        ),
        // MemAvailable and not MemFree: the kernel uses free RAM as cache,
        // and MemFree on a healthy Linux is almost zero -- showing it would
        // give the impression of a drowning machine.
        //
        // The values are stored and printed at END, not when the line
        // appears. Printing right away tied the output to the ORDER of
        // /proc/meminfo, where `MemTotal` comes BEFORE `MemAvailable`: the
        // two numbers came out glued together (`31.315.6`) and both fields
        // became `None`. The bug survived because this is the only Linux
        // machine and its detail was never queried -- nobody runs this
        // against itself over SSH. It showed up the moment the local card
        // started fetching details.
        HostOs::Linux => concat!(
            "awk '{printf \"%d|\", $1/60}' /proc/uptime; ",
            "awk '/^MemAvailable:/{a=$2} /^MemTotal:/{t=$2} ",
            "END{printf \"%.1f|%.1f|\", a/1048576, t/1048576}' /proc/meminfo; ",
            // Two samples: /proc/stat counts TIME accumulated since boot,
            // not current usage. A single read would give the machine's
            // lifetime average, which never changes and is useless.
            "s() { awk '/^cpu /{t=0;for(i=2;i<=NF;i++)t+=$i;print t\" \"($5+$6)}' /proc/stat; }; ",
            "a=$(s); sleep 0.2; b=$(s); ",
            "echo \"$a $b\" | awk '{d=$3-$1; i=$4-$2; ",
            "if(d>0) printf \"%.0f\", (d-i)*100/d; else printf \"0\"}'; ",
            // `-P` keeps each filesystem on one line, whatever the device
            // name's length. Available ($4), not free: root's reserved
            // blocks are space this user cannot write.
            "df -Pk / | awk 'NR==2{printf \"|%.2f|%.1f\", $4/1048576, $2/1048576}'"
        ),
        // macOS has no /proc. `kern.boottime` gives the boot epoch, and
        // the free pages come from vm_stat (free + inactive, which is what
        // is actually available).
        HostOs::Macos => concat!(
            // ANCHORED at the start. The output is
            //   `{ sec = 1793736815, usec = 137720 } Tue Nov  3 ...`
            // and a greedy `.*sec = ` matches up to `usec = `, capturing
            // 137720 -- the uptime became 56 years, a number big enough to
            // look like a unit bug and not a parsing one.
            "b=$(sysctl -n kern.boottime | sed -n 's/^{ sec = \\([0-9]*\\).*/\\1/p'); ",
            "up=$(( ( $(date +%s) - b ) / 60 )); ",
            "ps=$(sysctl -n hw.pagesize); ",
            "fr=$(vm_stat | awk -v p=$ps '/Pages free/{f=$3} /Pages inactive/{i=$3} ",
            "END{gsub(/\\./,\"\",f); gsub(/\\./,\"\",i); printf \"%.1f\", (f+i)*p/1073741824}'); ",
            "tot=$(echo $(sysctl -n hw.memsize) | awk '{printf \"%.1f\", $1/1073741824}'); ",
            // No `top -l 2`: it waits ~1 s for the second sample and the
            // Mac's card would always be a second late. The sum of the
            // processes' %cpu divided by the cores already answers "is it
            // busy?" at no cost.
            "n=$(sysctl -n hw.ncpu); ",
            "cpu=$(ps -A -o %cpu | awk -v n=$n 'NR>1{s+=$1} ",
            "END{v=s/n; if(v>100)v=100; printf \"%.0f\", v}'); ",
            // On APFS `/` is the sealed system volume, and its "available"
            // is the container's free space, shared with the data volume:
            // the number that runs out.
            "d=$(df -Pk / | awk 'NR==2{printf \"%.2f|%.1f\", $4/1048576, $2/1048576}'); ",
            "echo \"$up|$fr|$tot|$cpu|$d\""
        ),
        HostOs::Unknown => return None,
    })
}

/// A host's uptime and RAM -- including THIS one.
///
/// It was `ssh_detail` and required `h.ssh`, so the local machine went
/// without: it has no SSH to itself. The card of whoever was looking at
/// the panel showed no uptime or RAM, which is precisely the easiest card
/// to check -- and a person noticed it before any check did.
///
/// What resolves local vs. remote is `remote::run`, which already existed
/// for that. One path for both cases: the command is picked by the
/// TARGET's OS, and running PowerShell against Ubuntu would fail
/// silently.
pub fn host_detail(h: &HostCfg) -> Detail {
    let Some(cmd) = detail_cmd(h.os()) else {
        return Detail::default();
    };
    // A remote host without `ssh` in the config cannot be queried.
    if !h.is_local() && h.ssh.is_none() {
        return Detail::default();
    }
    let Ok(text) = crate::remote::run(h, cmd, 3) else {
        return Detail::default();
    };
    let Some(line) = text.lines().filter(|l| l.contains('|')).next_back() else {
        return Detail::default();
    };
    let parts: Vec<&str> = line.trim().split('|').collect();
    // Three fields or more: CPU came later, and accepting the short line
    // makes a new field degrade instead of wiping the ones that already
    // worked.
    if parts.len() < 3 {
        return Detail::default();
    }
    let field = |i: usize| parts.get(i).and_then(|v| v.trim().parse::<f64>().ok());
    let (disk_free_gb, disk_total_gb) = (field(4), field(5));
    Detail {
        uptime_min: parts[0].trim().parse::<f64>().ok().map(|v| v as u64),
        ram_free_gb: parts[1].trim().parse().ok(),
        ram_total_gb: parts[2].trim().parse().ok(),
        cpu_pct: field(3).map(|v| v.clamp(0.0, 100.0)),
        disk_free_gb,
        disk_total_gb,
        disk_level: crate::disk::note(&h.name, disk_free_gb, disk_total_gb),
    }
}

fn check_host(h: &HostCfg, cfg: &Config) -> HostState {
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

    // The local machine is online by construction: this code is running
    // ON it. Without this line, a host with no listed service shows "off"
    // forever, because the only evidence of life was "some port
    // answered". That is what happened when KDE Connect left the config:
    // the Ubuntu card had no services and the very machine serving the
    // panel showed up as off.
    //
    // For a REMOTE host without services the answer is still "off", and
    // that is right: we have no evidence at all that it exists.
    let online = h.is_local() || services.iter().any(|s| s.up);
    // Units ONLY for the local machine. `unit_state` queries the manager
    // here: doing that for a remote host would return the wrong machine's
    // state looking like the truth. Today no remote config lists units,
    // but the first one that did would lie without warning.
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
    // This machine always answers; the others, only when they are up --
    // querying a host that is off costs the whole timeout to return what
    // we already know.
    let detail = if h.is_local() || online { host_detail(h) } else { Detail::default() };

    // Only when it is up: an SSH call to a host that is off costs the
    // whole timeout and returns what we already know.
    let kvm = if h.is_local() || online { kvm_state(h, cfg) } else { Kvm::default() };

    HostState {
        name: h.name.clone(),
        addr: h.addr.clone(),
        role: h.role.clone(),
        online,
        services,
        units,
        detail,
        kvm,
    }
}

/// Reads a host's border and reconciler.
///
/// Local: straight from the in-memory config, which is the current
/// truth. Remote: through ITS daemon's `/api/layout`, over SSH. The
/// daemon listens only on 127.0.0.1 -- on purpose -- so the SSH tunnel is
/// the way, not opening the port on the LAN.
fn kvm_state(h: &HostCfg, cfg: &Config) -> Kvm {
    if h.is_local() {
        let b = &cfg.input.border;
        let f = &cfg.input.follow;
        return Kvm {
            border: Some(b.enabled),
            follow: Some(f.enabled),
            leader: f.leader.clone(),
            edges: cfg
                .border_edges()
                .into_iter()
                .map(|(e, t)| format!("{e} -> {t}"))
                .collect(),
            error: None,
        };
    }
    if h.ssh.is_none() {
        return Kvm::default();
    }
    // `curl` on Unix, `curl.exe` on Windows: there the SSH shell is
    // PowerShell, and `curl` is an ALIAS of `Invoke-WebRequest`, which
    // rejects the URL as a positional argument. Measured -- the alias
    // deceives.
    let prog = if crate::remote::is_powershell(h) { "curl.exe" } else { "curl" };
    let cmd = format!(
        "{prog} -s --max-time 4 http://127.0.0.1:{}/api/layout",
        cfg.port
    );
    let text = match crate::remote::run(h, &cmd, 6) {
        Ok(t) => t,
        Err(e) => return Kvm { error: Some(e), ..Default::default() },
    };
    let Some(line) = text.lines().find(|l| l.trim_start().starts_with('{')) else {
        return Kvm { error: Some("that machine's daemon did not answer JSON".into()), ..Default::default() };
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Kvm { error: Some("JSON ilegivel".into()), ..Default::default() };
    };
    Kvm {
        border: v.get("border_enabled").and_then(|x| x.as_bool()),
        follow: v.get("follow_enabled").and_then(|x| x.as_bool()),
        leader: v.get("follow_leader").and_then(|x| x.as_str()).map(String::from),
        edges: v
            .get("edges")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter(|e| e.get("ativa").and_then(|x| x.as_bool()) != Some(false))
                    .filter_map(|e| {
                        Some(format!(
                            "{} -> {}",
                            e.get("edge")?.as_str()?,
                            e.get("target")?.as_str()?
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        error: None,
    }
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let hosts = std::thread::scope(|s| {
        let handles: Vec<_> =
            cfg.hosts.iter().map(|h| s.spawn(|| check_host(h, cfg))).collect();
        handles.into_iter().filter_map(|x| x.join().ok()).collect()
    });
    Snapshot { generated: crate::sys::hhmmss(), hosts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    /// Reimplements the filter's criterion to pin it in a test -- the real
    /// function depends on name resolution, which does not fit in a unit
    /// test.
    fn link_local(a: &SocketAddr) -> bool {
        match a {
            SocketAddr::V4(v) => v.ip().octets()[..2] == [169, 254],
            SocketAddr::V6(v) => v.ip().segments()[0] & 0xffc0 == 0xfe80,
        }
    }

    #[test]
    fn link_local_is_discarded() {
        let v6_ll = SocketAddr::V6(SocketAddrV6::new(
            "fe80::cb7:7431:1bea:d882".parse::<Ipv6Addr>().unwrap(), 22, 0, 11));
        let v6_glob = SocketAddr::V6(SocketAddrV6::new(
            "2804:394:8b44:fe00:d1:763f:38e:9eb6".parse::<Ipv6Addr>().unwrap(), 22, 0, 0));
        let v4_ll = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(169, 254, 3, 7), 22));
        let v4 = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 147), 22));

        // The address that broke SFTP to the Mac.
        assert!(link_local(&v6_ll), "fe80::/10 is link-local");
        assert!(link_local(&v4_ll), "169.254/16 is link-local");
        assert!(!link_local(&v6_glob), "global IPv6 works -- measured with the Mac");

        assert!(!link_local(&v4));
    }
}
