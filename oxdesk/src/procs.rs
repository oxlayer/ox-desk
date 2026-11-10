//! Task manager: what is running on each machine, and killing it.
//!
//! Spec: ../specs/009-processes.md
//!
//! ## One command line per OS, picked by the TARGET's OS
//!
//! There is no command all three OSes understand. The choice comes from
//! `HostCfg::os`, not from the OS the daemon runs on -- running `ps`
//! against Windows, or `Get-Process` against Ubuntu, fails silently and
//! the panel stays empty without saying why.
//!
//! ## What `cpu` means changes per OS, and that is declared
//!
//! On Unix `ps` gives an instantaneous **percentage**. On Windows
//! `Get-Process` gives **accumulated seconds** of CPU since the process
//! started -- a number with another unit and another reading. Converting
//! one into the other would take two samples and is not worth the cost
//! here; so the field comes with `cpu_kind` saying which of the two it
//! is, and the UI labels it right. Showing "87" in a "%" column when it
//! is seconds would be a plausible lie, which is the worst kind.

use crate::config::HostOs;
use crate::remote;
use serde::Serialize;

/// Cap on returned processes. The list is sorted by CPU, so what matters
/// is at the top -- and 2000 lines of JSON help nobody.
const LIMIT: usize = 60;
const TIMEOUT_S: u32 = 8;

#[derive(Debug, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cpu: f64,
    pub mem_mb: f64,
}

#[derive(Debug, Serialize)]
pub struct Procs {
    pub host: String,
    /// local | ssh
    pub source: &'static str,
    /// pct | seconds -- see the module header.
    pub cpu_kind: &'static str,
    pub procs: Vec<Proc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Procs {
    fn failed(host: &str, source: &'static str, msg: impl Into<String>) -> Self {
        Procs {
            host: host.into(),
            source,
            cpu_kind: "pct",
            procs: vec![],
            error: Some(msg.into()),
        }
    }
}

fn list_cmd(os: HostOs) -> Option<(&'static str, &'static str)> {
    match os {
        // `comm=` without a header; sorting in `sort` because ps's sort
        // flag differs between Linux (`--sort`) and macOS (`-r`).
        HostOs::Linux | HostOs::Macos => Some((
            "ps -eo pid=,pcpu=,rss=,comm= | sort -k2 -rn | head -n 60",
            "pct",
        )),
        HostOs::Windows => Some((
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 60 \
             Id,ProcessName,CPU,WorkingSet64 | ConvertTo-Json -Compress",
            "seconds",
        )),
        HostOs::Unknown => None,
    }
}

pub fn list(cfg: &crate::config::Config, host: &str) -> Procs {
    let Some(h) = cfg.hosts.iter().find(|x| x.name == host) else {
        return Procs::failed(host, "local", format!("host '{host}' is not in the config"));
    };
    let source = if h.is_local() { "local" } else { "ssh" };
    let Some((cmd, cpu_kind)) = list_cmd(h.os()) else {
        return Procs::failed(
            host,
            source,
            "unknown host OS: declare `os` in [[hosts]] so I know which command to use",
        );
    };
    let output = match remote::run(h, cmd, TIMEOUT_S) {
        Ok(s) => s,
        Err(e) => return Procs::failed(host, source, e),
    };
    let mut procs = if remote::is_powershell(h) { parse_ps_json(&output) } else { parse_ps(&output) };
    procs.truncate(LIMIT);
    Procs { host: host.into(), source, cpu_kind, procs, error: None }
}

/// `  1234  12.5  65432 process name`
fn parse_ps(out: &str) -> Vec<Proc> {
    let mut v = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let (Ok(pid), Ok(cpu), Ok(rss)) =
            (f[0].parse::<u32>(), f[1].parse::<f64>(), f[2].parse::<f64>())
        else {
            continue;
        };
        // The name may contain spaces (an app path on macOS), so it is
        // everything after the third field. Only the last component stays:
        // the whole path of a `.app` does not fit on screen and says
        // nothing.
        let raw = f[3..].join(" ");
        let name = raw.rsplit('/').next().unwrap_or(&raw).to_string();
        v.push(Proc { pid, name, cpu, mem_mb: (rss / 1024.0 * 10.0).round() / 10.0 });
    }
    v
}

fn parse_ps_json(out: &str) -> Vec<Proc> {
    // A single process makes ConvertTo-Json return an object, not a list.
    let Some(start) = out.find(['[', '{']) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(out[start..].trim()) else {
        return Vec::new();
    };
    let items = match &v {
        serde_json::Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    items
        .iter()
        .filter_map(|p| {
            let pid = p.get("Id")?.as_u64()? as u32;
            let name = p.get("ProcessName")?.as_str()?.to_string();
            // CPU comes as `null` for a process with no time accounted.
            let cpu = p.get("CPU").and_then(|c| c.as_f64()).unwrap_or(0.0);
            let ws = p.get("WorkingSet64").and_then(|w| w.as_f64()).unwrap_or(0.0);
            Some(Proc {
                pid,
                name,
                cpu: (cpu * 10.0).round() / 10.0,
                mem_mb: (ws / 1_048_576.0 * 10.0).round() / 10.0,
            })
        })
        .collect()
}

/// Ends a process. `force` swaps the polite request for the definitive
/// one.
///
/// Requires `confirm` for the same reason as `/api/focus` and `set-host`:
/// killing a process on someone's machine cannot happen from a distracted
/// click, and it has no undo.
pub fn kill(
    cfg: &crate::config::Config,
    host: &str,
    pid: u32,
    force: bool,
    confirm: bool,
) -> Result<String, String> {
    if !confirm {
        return Err("confirm is missing: killing a process has no undo".into());
    }
    if pid <= 1 {
        return Err("pid 0 e 1 sao do sistema; recusado".into());
    }
    let h = cfg
        .hosts
        .iter()
        .find(|x| x.name == host)
        .ok_or_else(|| format!("host '{host}' is not in the config"))?;

    let cmd = match h.os() {
        HostOs::Windows => {
            // `/T` takes the tree along: closing only the parent leaves an
            // orphan child running, and the user clicked "encerrar"
            // expecting the thing to go away.
            if force {
                format!("taskkill /PID {pid} /T /F")
            } else {
                format!("taskkill /PID {pid} /T")
            }
        }
        HostOs::Linux | HostOs::Macos => {
            let signal = if force { "KILL" } else { "TERM" };
            format!("kill -{signal} {pid}")
        }
        HostOs::Unknown => {
            return Err("unknown host OS: declare `os` in [[hosts]]".into())
        }
    };
    let output = remote::run(h, &cmd, TIMEOUT_S)?;
    let mode = if force { "forcado" } else { "educado" };
    say!("procs: ended pid {pid} on {host} ({})", if force { "forced" } else { "polite" });
    Ok(if output.trim().is_empty() { format!("pid {pid} encerrado ({mode})") } else { output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_ps() {
        let out = "\
 1234  12.5  65432 /usr/lib/firefox/firefox
   42   0.0   1024 kworker/0:1
  777   3.2 204800 /Applications/Visual Studio Code.app/Contents/MacOS/Electron
";
        let v = parse_ps(out);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].pid, 1234);
        assert_eq!(v[0].name, "firefox", "only the last path component stays");
        assert_eq!(v[0].cpu, 12.5);
        assert_eq!(v[0].mem_mb, 63.9, "rss comes in KiB");
        // A path with spaces must not truncate the name.
        assert_eq!(v[2].name, "Electron");
    }

    #[test]
    fn parses_windows_get_process() {
        let out = r#"[{"Id":4321,"ProcessName":"chrome","CPU":87.421875,"WorkingSet64":524288000},
                      {"Id":9,"ProcessName":"idle","CPU":null,"WorkingSet64":8192}]"#;
        let v = parse_ps_json(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "chrome");
        assert_eq!(v[0].cpu, 87.4);
        assert_eq!(v[0].mem_mb, 500.0);
        // A null CPU must not drop the whole row.
        assert_eq!(v[1].cpu, 0.0);
    }

    #[test]
    fn single_object_also_parses() {
        // With ONE process ConvertTo-Json returns an object, not a list.

        let v = parse_ps_json(r#"{"Id":1,"ProcessName":"so","CPU":1.0,"WorkingSet64":1048576}"#);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].mem_mb, 1.0);
    }
}
