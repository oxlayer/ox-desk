//! File browsing: local through `std::fs`, remote through SFTP.
//!
//! Spec: ../specs/003-files.md
//!
//! ## Why SFTP and not a subcommand on the agent
//!
//! The SSH channel to the peer already exists and is already trusted --
//! it is how the daemon probes the agent and fires the dead-man's switch.
//! SFTP runs on that same channel, with the same key and the same
//! authorization, and `sftp-server` ships with OpenSSH on both OSes. A new
//! subcommand on the agent would mean more surface to maintain and nothing
//! more in return.
//!
//! ## Copy yes, delete no
//!
//! The daemon is a privilege surface (spec 008), and writing by accident
//! from a panel I serve myself is the kind of error with no undo. That is
//! why writing is narrow: only copy, between this machine and another, and
//! never over something that already exists. Delete and move stay out.
//!
//! ## Paths are not comparable between the two sources
//!
//! SFTP exposes the Windows disk as `/C:/Users/Alice`; `std::fs` on the
//! machine itself says `C:\Users\Alice`. Two names for the same place,
//! and neither is wrong. Instead of normalizing (and getting it wrong),
//! each listing declares its `source`, returns an opaque `path` for the UI
//! to navigate with, and a separate `display` to show people.

use crate::config::Config;
use serde::Serialize;
use std::sync::Mutex;
use std::io::Write;
use std::process::Stdio;

/// Cap on entries per listing. A folder with 50 thousand files helps
/// nobody on screen and bloats the JSON.
const MAX_ENTRIES: usize = 2000;
const SFTP_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Serialize)]
pub struct Entry {
    pub name: String,
    /// dir | file | link | other
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Unix epoch, when the source can tell (local path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<u64>,
    /// Text as the source delivered it, when it gives no epoch (SFTP).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_text: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub host: String,
    /// Canonical path to navigate with -- opaque, send back as received.
    pub path: String,
    /// How to show it on screen.
    pub display: String,
    /// local | sftp
    pub source: &'static str,
    /// Parent path, or `None` at the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    /// Went over `MAX_ENTRIES` and was cut.
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Listing {
    fn failed(host: &str, path: &str, source: &'static str, msg: impl Into<String>) -> Self {
        Listing {
            host: host.into(),
            path: path.into(),
            display: path.into(),
            source,
            parent: None,
            entries: vec![],
            truncated: false,
            error: Some(msg.into()),
        }
    }
}

/// Decodes the query's percent-encoding. No crate: it is a one-table
/// parser, and `path` arrives with `/`, `\`, `:` and spaces.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Lists `path` on `host`. Empty `path` = that host's starting point.
pub fn list(cfg: &Config, host: &str, path: &str) -> Listing {
    let Some(h) = cfg.hosts.iter().find(|x| x.name == host) else {
        return Listing::failed(host, path, "local", format!("host '{host}' is not in the config"));
    };
    if h.is_local() {
        return list_local(host, path);
    }
    let Some(alias) = h.ssh.as_deref() else {
        return Listing::failed(
            host,
            path,
            "sftp",
            format!(
                "host '{host}' has no `ssh` in the config: with no SSH channel \
                 there is no way to list. On Linux `sshd` may not be \
                 installed yet -- see specs/003."
            ),
        );
    };
    list_sftp(host, alias, &h.addr, path)
}

// --- local -----------------------------------------------------------------

fn list_local(host: &str, path: &str) -> Listing {
    let p = if path.is_empty() {
        crate::sys::home_dir()
    } else {
        std::path::PathBuf::from(path)
    };
    let dir = match std::fs::read_dir(&p) {
        Ok(d) => d,
        Err(e) => {
            return Listing::failed(host, &p.to_string_lossy(), "local", format!("{e}"));
        }
    };

    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    for e in dir.flatten() {
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        // `metadata()` follows links; `file_type()` does not. We want to
        // know it IS a link, and still be able to say whether it points to
        // a folder.
        let ft = e.file_type().ok();
        let md = e.metadata().ok();
        let kind = match (&ft, &md) {
            (Some(t), _) if t.is_symlink() => "link",
            (_, Some(m)) if m.is_dir() => "dir",
            (_, Some(m)) if m.is_file() => "file",
            _ => "other",
        };
        let size = md.as_ref().filter(|m| m.is_file()).map(|m| m.len());
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(Entry { name, kind, size, mtime, mtime_text: None });
    }
    sort_entries(&mut entries);

    let canon = p.to_string_lossy().into_owned();
    Listing {
        host: host.into(),
        path: canon.clone(),
        display: canon,
        source: "local",
        parent: p.parent().map(|x| x.to_string_lossy().into_owned()),
        entries,
        truncated,
        error: None,
    }
}

// --- remote over SFTP ------------------------------------------------------

/// Runs a batch in `sftp` and returns its stdout.
///
/// `-b -` reads the commands from stdin. Batch instead of an interactive
/// session because the daemon has nobody to talk to: one request, one
/// response.
fn sftp_batch(alias: &str, addr: &str, script: &str) -> Result<String, String> {
    let mut args: Vec<String> = vec![
        "-q".into(),
        "-b".into(),
        "-".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={SFTP_TIMEOUT_SECS}"),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ];
    // Same workaround as `agent.rs`: we resolve here, because the resolver
    // of the ssh/sftp the daemon launches on Windows does not see the name.
    // See `probe::ssh_addr_opts`.
    args.extend(crate::probe::ssh_addr_opts(addr));
    args.push(alias.to_string());
    let mut child = crate::sys::child("sftp")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run sftp: {e}"))?;

    child
        .stdin
        .as_mut()
        .ok_or("sftp has no stdin")?
        .write_all(script.as_bytes())
        .map_err(|e| format!("escrever no sftp: {e}"))?;
    drop(child.stdin.take());

    let out = child.wait_with_output().map_err(|e| format!("esperar sftp: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { "sftp failed with no message".into() } else { err });
    }
    Ok(stdout)
}

fn list_sftp(host: &str, alias: &str, addr: &str, path: &str) -> Listing {
    // ONE batch, not two. The first version called `pwd` and then
    // `cd`+`ls`, paying two SSH handshakes -- 809 ms measured to list a
    // folder. The `pwd` in the same batch gives the canonical path for
    // free, and `display` and `parent` come from it: so the UI navigates
    // by what the server confirms, not by what I assumed.
    //
    // Single quotes in the `cd`: names with spaces are common (`C:/Program
    // Files`). Quotes in the input are removed instead of escaped -- a file
    // name with single quotes is rare enough not to be worth the
    // complexity, and the worst case is the listing failing, not a command
    // leaking.
    let script = if path.is_empty() {
        "pwd\nls -la\n".to_string()
    } else {
        format!("cd '{}'\npwd\nls -la\n", path.replace('\'', ""))
    };
    let out = match sftp_batch(alias, addr, &script) {
        Ok(o) => o,
        Err(e) => return Listing::failed(host, path, "sftp", e),
    };
    let Some(canon) = parse_pwd(&out) else {
        return Listing::failed(
            host,
            path,
            "sftp",
            if path.is_empty() {
                "sftp did not answer pwd".to_string()
            } else {
                format!("could not enter '{path}'")
            },
        );
    };

    let (mut entries, truncated) = parse_ls_la(&out);
    sort_entries(&mut entries);

    Listing {
        host: host.into(),
        path: canon.clone(),
        display: windows_display(&canon),
        source: "sftp",
        parent: sftp_parent(&canon),
        entries,
        truncated,
        error: None,
    }
}

fn parse_pwd(out: &str) -> Option<String> {
    // "Remote working directory: /C:/Users/Alice"
    out.lines()
        .find_map(|l| l.split_once("working directory:").map(|(_, p)| p.trim().to_string()))
        .filter(|p| !p.is_empty())
}

/// Parent of an SFTP path. `/C:/` is the disk root: above it `sftp`
/// lists nothing useful, so we stop there.
fn sftp_parent(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    if idx == 0 {
        // "/C:" -> we do not go up to "/"
        return None;
    }
    Some(trimmed[..idx].to_string())
}

/// `/C:/Users/Alice` -> `C:\Users\Alice`. For the eyes, not for
/// navigating: navigation uses `path`.
fn windows_display(path: &str) -> String {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[2] == b':' && (b[1] as char).is_ascii_alphabetic() {
        return path[1..].replace('/', "\\");
    }
    path.to_string()
}

/// Parses sftp's `ls -la` output.
///
/// Format (the server's, not ours):
///   `drwxrwxrwx   1 Alice Alice  0 Nov  3 22:00 Documents`
///
/// **Anchored on the month, not on a field count.** The first version
/// counted: name = after the 8th field. Wrong -- there are nine (mode,
/// links, owner, group, size, month, day, time, name), and the test caught
/// it: `.` and `..` got through the filter because the name read was
/// another piece of the line. Fixing the index to 9 would not be enough:
/// some servers omit the group, and then there are eight. Looking for the
/// month finds the joint in either case, because from there rightwards the
/// format is fixed: `<month> <day> <time-or-year> <name>`, and the size is
/// the field right before.
fn parse_ls_la(out: &str) -> (Vec<Entry>, bool) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut entries = Vec::new();
    let mut truncated = false;
    for line in out.lines() {
        let line = line.trim_end();
        // sftp's own echo lines and ls totals.
        if line.starts_with("sftp>") || line.starts_with("total ") || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let mode = fields[0];
        if mode.len() < 10 || !matches!(mode.as_bytes()[0], b'd' | b'-' | b'l') {
            continue;
        }
        // The month cannot be field 0 or 1; starting at 2 avoids matching
        // an owner called "Mar".
        let Some(m) = (2..fields.len().saturating_sub(2))
            .find(|&i| MONTHS.contains(&fields[i]) && fields[i + 1].parse::<u8>().is_ok())
        else {
            continue;
        };
        let size = fields[m - 1].parse::<u64>().ok();
        let when = format!("{} {} {}", fields[m], fields[m + 1], fields[m + 2]);

        // The name is everything after the time field -- positional on
        // purpose, because names with spaces are common (`Program Files`)
        // and joining the remaining fields would collapse repeated spaces.
        let name = match nth_field_offset(line, m + 4) {
            Some(i) => line[i..].to_string(),
            None => continue,
        };
        if name == "." || name == ".." {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        let kind = match mode.as_bytes()[0] {
            b'd' => "dir",
            b'l' => "link",
            b'-' => "file",
            _ => "other",
        };
        entries.push(Entry {
            name,
            kind,
            size: if kind == "file" { size } else { None },
            mtime: None,
            mtime_text: Some(when),
        });
    }
    (entries, truncated)
}

/// Byte where the n-th space-separated field (1-based) starts.
fn nth_field_offset(line: &str, n: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut i = 0;
    let mut field = 0;
    while i < b.len() {
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        field += 1;
        if field == n {
            return Some(i);
        }
        while i < b.len() && b[i] != b' ' {
            i += 1;
        }
    }
    None
}

/// Folders before files, and within each group by case-insensitive
/// name -- the order the person expects, not the OS's.
fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        let rank = |k: &str| match k {
            "dir" => 0,
            "link" => 1,
            _ => 2,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

// --- transfer --------------------------------------------------------------

/// A copy requested from the screen. `state` is `running`, `ok` or
/// `error` -- wire values the page reads as CSS classes.
#[derive(Debug, Clone, Serialize)]
pub struct Transfer {
    pub id: u64,
    pub name: String,
    pub from: String,
    pub to: String,
    pub state: &'static str,
    pub detail: String,
}

static TRANSFERS: Mutex<Vec<Transfer>> = Mutex::new(Vec::new());
/// How many stay in the screen's list. The copy itself does not depend on it.
const MAX_TRANSFERS: usize = 30;

pub fn transfers() -> Vec<Transfer> {
    TRANSFERS.lock().map(|g| g.clone()).unwrap_or_default()
}

fn finish(id: u64, state: &'static str, detail: String) {
    if let Ok(mut g) = TRANSFERS.lock() {
        if let Some(t) = g.iter_mut().find(|t| t.id == id) {
            t.state = state;
            t.detail = detail;
        }
    }
}

/// Refuses what does not fit in quotes in the sftp batch, instead of
/// silently altering the name and copying something else. `*?[` would be
/// a glob in `get`.
fn safe_path(c: &str) -> Result<(), String> {
    match c.chars().find(|x| matches!(x, '\'' | '"' | '\n' | '\r' | '*' | '?' | '[')) {
        Some(x) => Err(format!("the path has a character I cannot copy: {x:?}")),
        None => Ok(()),
    }
}

/// Joins folder and name with `/`: sftp uses it on both systems.
fn join(folder: &str, name: &str) -> String {
    format!("{}/{name}", folder.trim_end_matches(['/', '\\']))
}

fn name_of(path: &str) -> Option<&str> {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|n| !n.is_empty())
}

fn entry_in(cfg: &Config, host: &str, folder: &str, name: &str) -> Result<Option<Entry>, String> {
    let l = list(cfg, host, folder);
    if let Some(e) = l.error {
        return Err(e);
    }
    Ok(l.entries.into_iter().find(|e| e.name == name))
}

/// Copies `path` from `from` to folder `folder` on `to`. Returns the id,
/// and the copy goes on in a thread: a large file must not hold the server.
pub fn transfer(
    cfg: &Config,
    from: &str,
    path: &str,
    to: &str,
    folder: &str,
) -> Result<u64, String> {
    let find = |n: &str| {
        cfg.hosts.iter().find(|h| h.name == n).ok_or_else(|| format!("host '{n}' is not in the config"))
    };
    let (ho, hd) = (find(from)?, find(to)?);
    let remote = match (ho.is_local(), hd.is_local()) {
        (true, false) => hd,
        (false, true) => ho,
        _ => return Err("one of the two ends has to be this machine".into()),
    };
    let alias = remote.ssh.clone().ok_or_else(|| format!("'{}' has no `ssh`", remote.name))?;
    safe_path(path)?;
    safe_path(folder)?;
    let name = name_of(path).ok_or("path with no name")?.to_string();
    let parent = path.trim_end_matches(['/', '\\']);
    let parent = parent[..parent.len() - name.len()].to_string();

    let source = entry_in(cfg, from, &parent, &name)?.ok_or("the file is not there any more")?;
    if entry_in(cfg, to, folder, &name)?.is_some() {
        return Err(format!("'{name}' already exists at the destination: nothing was copied"));
    }

    let dest = join(folder, &name);
    let command = if ho.is_local() { "put" } else { "get" };
    let script = format!("{command} -r '{path}' '{dest}'\n");
    let mut g = TRANSFERS.lock().map_err(|_| "lista travada")?;
    let id = g.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    g.push(Transfer {
        id,
        name: name.clone(),
        from: from.into(),
        to: to.into(),
        state: "running",
        detail: String::new(),
    });
    let excess = g.len().saturating_sub(MAX_TRANSFERS);
    g.drain(..excess);
    drop(g);

    let (cfg, to, folder, addr) =
        (cfg.clone(), to.to_string(), folder.to_string(), remote.addr.clone());
    std::thread::spawn(move || {
        if let Err(e) = sftp_batch(&alias, &addr, &script) {
            return finish(id, "error", e);
        }
        // Finishing is not arriving: the destination is what answers.
        match entry_in(&cfg, &to, &folder, &name) {
            Ok(Some(e)) if source.kind != "file" || e.size == source.size => {
                let t = e.size.map(|b| format!("{b} bytes")).unwrap_or_else(|| "folder".into());
                say!("files: {name} copied to {to}");
                finish(id, "ok", t);
            }
            Ok(Some(e)) => finish(
                id,
                "error",
                format!("chegou com {:?} bytes, esperava {:?}", e.size, source.size),
            ),
            Ok(None) => finish(id, "error", "the destination does not have the file".into()),
            Err(e) => finish(id, "error", e),
        }
    });
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_with_quotes_or_glob_is_refused() {
        assert!(safe_path("/C:/Users/Alice/My Files/a.txt").is_ok());
        assert!(safe_path("/tmp/a'b").is_err());
        assert!(safe_path("/tmp/*").is_err());
        assert!(safe_path("/tmp/a\nb").is_err());
    }

    #[test]
    fn name_comes_from_the_end_of_the_path() {
        assert_eq!(name_of("/C:/Users/a.txt"), Some("a.txt"));
        assert_eq!(name_of("C:\\Users\\folder\\"), Some("folder"));
        assert_eq!(name_of("/"), None);
        assert_eq!(join("/tmp/", "a"), "/tmp/a");
    }


    #[test]
    fn decodes_percent_and_plus() {
        assert_eq!(percent_decode("%2FC%3A%2FProgram%20Files"), "/C:/Program Files");
        assert_eq!(percent_decode("a+b"), "a b");
        // A stray `%` must not bring the parser down.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn parses_sftp_ls_la() {
        let out = "\
sftp> cd '/C:/Users/Alice'
sftp> ls -la
drwxrwxrwx    1 Alice Alice        0 Nov  3 22:00 .
drwxrwxrwx    1 Alice Alice        0 Oct 26 14:05 ..
drwxrwxrwx    1 Alice Alice        0 Nov  3 21:12 Program Files
-rw-rw-rw-    1 Alice Alice     1234 Nov  3 22:19 watch-edge.log
lrwxrwxrwx    1 Alice Alice       12 Nov  1 09:00 shortcut
total 4
";
        let (mut v, truncated) = parse_ls_la(out);
        sort_entries(&mut v);
        assert!(!truncated);
        // `.` and `..` go away; the parent comes from the path, not the listing.
        assert_eq!(v.len(), 3);
        // Folders first, then link, then file.
        assert_eq!(v[0].name, "Program Files", "a name with a space has to survive");
        assert_eq!(v[0].kind, "dir");
        assert_eq!(v[1].kind, "link");
        assert_eq!(v[2].name, "watch-edge.log");
        assert_eq!(v[2].size, Some(1234));
        assert_eq!(v[2].mtime_text.as_deref(), Some("Nov 3 22:19"));
    }

    #[test]
    fn parses_even_without_the_group_field() {
        // Some SFTP servers omit the group: eight fields instead of nine.
        // A fixed count would break here; the month anchor does not.
        let out = "-rw-r--r--    1 alice    4096 Nov  3 22:00 final note.txt\n";
        let (v, _) = parse_ls_la(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "final note.txt");
        assert_eq!(v[0].size, Some(4096));
    }

    #[test]
    fn year_instead_of_time_for_old_file() {
        // `ls` swaps the time for the year when the file is over 6 months old.
        let out = "-rw-r--r--    1 v v    10 Jan  2  2025 velho.txt\n";
        let (v, _) = parse_ls_la(out);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "velho.txt");
        assert_eq!(v[0].mtime_text.as_deref(), Some("Jan 2 2025"));
    }

    #[test]
    fn windows_path_becomes_readable_but_navigable() {
        assert_eq!(windows_display("/C:/Users/Alice"), "C:\\Users\\Alice");
        // A Unix path stays as it is.
        assert_eq!(windows_display("/home/alice"), "/home/alice");
    }

    #[test]
    fn parent_stops_at_the_disk_root() {
        assert_eq!(sftp_parent("/C:/Users/Alice"), Some("/C:/Users".into()));
        assert_eq!(sftp_parent("/C:/Users"), Some("/C:".into()));
        // Above `/C:` sftp lists nothing useful.
        assert_eq!(sftp_parent("/C:"), None);
        assert_eq!(sftp_parent("/"), None);
    }

    #[test]
    fn sftp_pwd() {
        assert_eq!(
            parse_pwd("Remote working directory: /C:/Users/Alice\n").as_deref(),
            Some("/C:/Users/Alice")
        );
        assert_eq!(parse_pwd("nothing here"), None);
    }
}
