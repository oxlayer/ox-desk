//! Config in TOML, readable and editable by hand.
//!
//! Two levels kept apart on purpose (spec 001): config is what the user
//! chose, runtime is what was discovered. Only config lives here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How to interpret a closed port. Three states because two gave false
/// alarms: the SSH that does not exist here on purpose, and the Windows
/// RDP turned off by choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// Must be up; red if it goes down.
    #[default]
    Up,
    /// Must be closed (e.g. a server port on this machine, which is a
    /// client).
    Down,
    /// Informational; never alarms.
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceCfg {
    pub label: String,
    pub port: u16,
    #[serde(default)]
    pub expect: Expect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCfg {
    pub name: String,
    /// mDNS name, **never an IP**. See spec 007.
    pub addr: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub services: Vec<ServiceCfg>,
    /// systemd --user units to inspect (only makes sense on the local
    /// machine).
    #[serde(default)]
    pub units: Vec<String>,
    /// Alias in ~/.ssh/config, if details can be fetched over SSH.
    #[serde(default)]
    pub ssh: Option<String>,
    /// Path of oxdesk-agent on this machine. Without it there is no
    /// dead-man's switch -- and without a dead-man's switch the physical
    /// route is one-way.
    #[serde(default)]
    pub agent: Option<String>,
    /// Where this machine sits on the GUI grid. See `Config::neighbors`.
    #[serde(default)]
    pub layout: Option<Cell>,
    /// windows | linux | macos. Optional: see `HostCfg::os`.
    #[serde(default)]
    pub os: Option<String>,
    /// How to control this host's screen, when possible.
    #[serde(default)]
    pub screen: Option<ScreenCfg>,
    /// Is this entry the machine the daemon runs on?
    ///
    /// It used to be implicit -- `addr = "127.0.0.1"` and free text in
    /// `role` saying "este PC". It started to matter when the daemon went
    /// to run on Windows: the dead-man's switch needs to know where to
    /// give the device back, and "the host that is not me" requires
    /// knowing who I am.
    ///
    /// Optional: without the key, the loopback-address heuristic applies,
    /// so existing configs keep working.
    #[serde(default)]
    pub local: Option<bool>,
}

impl HostCfg {
    /// This host's OS. The `os` key rules; without it, we infer.
    ///
    /// The inference exists for configs already written: the local host
    /// is the OS this binary runs on, and an `agent` with `%VAR%` or `\\`
    /// can only be a Windows path. When it cannot be known, `Unknown` --
    /// and callers degrade instead of guessing and running the wrong
    /// command on someone else's machine.
    pub fn os(&self) -> HostOs {
        if let Some(v) = &self.os {
            return match v.to_ascii_lowercase().as_str() {
                "windows" | "win" => HostOs::Windows,
                "linux" => HostOs::Linux,
                "macos" | "mac" | "darwin" => HostOs::Macos,
                _ => HostOs::Unknown,
            };
        }
        if self.is_local() {
            return if cfg!(windows) {
                HostOs::Windows
            } else if cfg!(target_os = "macos") {
                HostOs::Macos
            } else {
                HostOs::Linux
            };
        }
        match &self.agent {
            Some(a) if a.contains('%') || a.contains('\\') => HostOs::Windows,
            _ => HostOs::Unknown,
        }
    }

    pub fn is_local(&self) -> bool {
        if let Some(v) = self.local {
            return v;
        }
        matches!(self.addr.as_str(), "127.0.0.1" | "::1" | "localhost")
    }
}

/// The host's position on the grid.
///
/// Writing `edge = "right"` and `target = "windows-pc"` by hand was always
/// describing a SPATIAL relation in prose. The grid says the same thing
/// the way the person already thinks -- "Windows is to the right" -- and
/// the edges come out of it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Cell {
    pub col: i32,
    pub row: i32,
}

/// The host's operating system.
///
/// It started to matter when the peer stopped always being Windows. Two
/// places build a command to run ON the peer, and both are specific:
/// `agent::devices` builds PowerShell (`& "exe" devices`) and
/// `probe::host_detail` runs a PowerShell one-liner with CIM. Against Ubuntu
/// or macOS both would fail -- and fail silently, because the detail is
/// decoration and the agent probe already degrades to "did not answer".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    Linux,
    Macos,
    Unknown,
}

/// The host's screen protocol, and on which port.
///
/// One per host because it is a property of the OS: Windows speaks RDP,
/// macOS speaks VNC (Screen Sharing), GNOME speaks RDP through
/// `gnome-remote-desktop`. There is no protocol all three serve, and
/// picking each one's native protocol avoids starting a new server on any
/// of them -- the decision is in specs/006.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenCfg {
    /// rdp | vnc
    pub kind: String,
    #[serde(default = "default_screen_port")]
    pub port: u16,
    /// User for the client to prefill. It is not a credential: a password
    /// never goes into this config.
    #[serde(default)]
    pub user: Option<String>,
}

fn default_screen_port() -> u16 {
    3389
}

fn default_edge() -> String {
    "right".into()
}

/// Border watch. **Off by default** on purpose: it triggers a channel
/// switch, which is irreversible on the origin side (spec 005).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BorderCfg {
    #[serde(default)]
    pub enabled: bool,
    /// left | right | top | bottom
    #[serde(default = "default_edge")]
    pub edge: String,
    /// Host that receives focus on crossing. Without it, it does not arm.
    #[serde(default)]
    pub target: Option<String>,
    /// How many activations within the window before switching. `1` =
    /// immediate.
    ///
    /// Mutter fires on contact and has no dwell; we approximate it by
    /// activation density. Higher avoids switching on a graze, lower
    /// responds right away.
    #[serde(default = "default_hold_hits")]
    pub hold_hits: usize,
    #[serde(default = "default_hold_window_ms")]
    pub hold_window_ms: u64,
}

fn default_hold_hits() -> usize {
    2
}
fn default_hold_window_ms() -> u64 {
    300
}

impl Default for BorderCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: default_edge(),
            target: None,
            hold_hits: default_hold_hits(),
            hold_window_ms: default_hold_window_ms(),
        }
    }
}

fn default_follow_interval() -> u64 {
    5
}

/// Make the other devices follow the leader's channel.
///
/// Solves the split state at the root: mouse and keyboard on different
/// machines. And it works even when the switch was **manual**, with the
/// Easy-Switch button -- which `border` does not see.
///
/// **Off by default**: moving a device is irreversible on the origin side
/// (spec 005), and a reconciler that gets it wrong leaves you without a
/// keyboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowCfg {
    #[serde(default)]
    pub enabled: bool,
    /// (HID++) name of the leading device. The others in `[[devices]]`
    /// follow. Without it, it does not arm.
    #[serde(default)]
    pub leader: Option<String>,
    #[serde(default = "default_follow_interval")]
    pub interval_seconds: u64,
}

impl Default for FollowCfg {
    fn default() -> Self {
        Self { enabled: false, leader: None, interval_seconds: default_follow_interval() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InputCfg {
    /// How to move input between machines: `"lan"` or `"hidpp"`.
    ///
    /// - **`lan`** (default): capture here (`EVIOCGRAB`) and send it over
    ///   the remote screen's input stream. The device stays put, it does
    ///   not matter who has the dongle, and it is reversible on the spot.
    /// - **`hidpp`**: tells the device ITSELF to switch Easy-Switch
    ///   channel (0x1814). Requires a paired receiver on the target
    ///   machine, and is irreversible from the origin -- after ChangeHost
    ///   this receiver no longer reaches the device.
    ///
    /// The default became `lan` when we were down to a single dongle:
    /// with `hidpp`, "send to windows-pc" made the keyboard look for a
    /// receiver that does not exist there, not find it, and come back.
    /// Correct protocol behavior solving the wrong problem.
    ///
    /// Capture only exists on Linux for now.
    #[serde(default = "default_input_mode")]
    pub mode: String,
    #[serde(default)]
    pub border: BorderCfg,
    #[serde(default)]
    pub follow: FollowCfg,
}

fn default_input_mode() -> String {
    "lan".into()
}

fn default_port() -> u16 {
    8787
}
fn default_refresh() -> u64 {
    10
}

/// A Logitech device and which Easy-Switch channel it lives on for each
/// host.
///
/// This map is exactly what Solaar lacks and what Logitech Flow keeps in
/// the cloud. Here it lives in a TOML of your own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCfg {
    /// Name as Solaar reports it (e.g. "MX Anywhere 3").
    pub name: String,
    /// host -> channel (1-based).
    #[serde(default)]
    pub hosts: BTreeMap<String, u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub hosts: Vec<HostCfg>,
    #[serde(default)]
    pub devices: Vec<DeviceCfg>,
    #[serde(default)]
    pub input: InputCfg,
    /// Session telemetry (spec 017): one local line per screen session, with
    /// the path and how it went, never who or what. On unless set to false;
    /// `OXDESK_TELEMETRY=0` in the environment also turns it off.
    #[serde(default = "default_true")]
    pub telemetry: bool,
}

fn default_true() -> bool {
    true
}

/// Template for the first-run config -- **not** a config.
///
/// It carries `@NAME@` and `@OS@` because a default is inheritance:
/// whatever stands in this file is what a stranger's first run adopts. Up
/// to 2026-11-10 it named three real machines, their mDNS addresses and
/// their SSH aliases, which leaked in both directions -- our names out,
/// and a new user's setup wrong. See `.claude/rules/no-personal-data.md`
/// and `specs/018-open-source.md` step 4.
///
/// Write it through [`initial_config`], never directly.
pub const DEFAULT_TOML: &str = include_str!("../default-config.toml");

/// The config the first run writes, for the machine it runs on.
///
/// Pure, with the host's name passed in, so the substitution is tested
/// without needing a machine of a given name to read it from -- which is
/// the half of "a fresh config describes THIS machine" that a test can
/// actually hold.
pub fn initial_config(template: &str, hostname: &str, os: &str) -> String {
    // An empty hostname does not become `name = ""`: the panel keys hosts
    // by name and the local machine would be unaddressable. A visible
    // placeholder is better than a valid-looking empty string.
    let name = hostname.trim();
    let name = if name.is_empty() { "this-machine" } else { name };
    let out = template.replace("@NAME@", name).replace("@OS@", os);
    // systemd only. The template carries the empty list, so Windows and
    // macOS need no substitution at all -- and a unit that does not exist
    // there would answer "inactive" forever, showing the machine the panel
    // runs on as switched off.
    if os == "linux" {
        out.replace("units = []", "units = [\"oxdesk.service\"]")
    } else {
        out
    }
}

/// `linux` | `windows` | `macos`, for this build.
pub fn local_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// This machine's name, for the first-run config.
///
/// `hostname` exists on all three systems; the environment is the fallback
/// for an image stripped down enough not to have it. Neither is fatal --
/// `initial_config` turns an empty answer into a placeholder.
pub fn local_hostname() -> String {
    if let Ok(out) = std::process::Command::new("hostname").output() {
        if let Ok(s) = String::from_utf8(out.stdout) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// Replaces values inside a section, preserving the rest of the file.
///
/// The third time this surgery showed up (grid, border, now the
/// reconciler), so it became a function. The reason is always the same:
/// reserializing the struct produces correct TOML and wipes the comments,
/// and in this config they are the record of why each choice was made.
///
/// A key that does not exist in the section is **appended** at its end,
/// not ignored -- `leader` may never have been written.
pub fn set_in_section(toml_text: &str, section: &str, pairs: &[(&str, String)]) -> String {
    let header = format!("[{section}]");
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut seen: Vec<&str> = Vec::new();

    let missing_keys = |seen: &Vec<&str>| -> Vec<String> {
        pairs
            .iter()
            .filter(|(k, _)| !seen.contains(k))
            .map(|(k, v)| format!("{k} = {v}"))
            .collect()
    };

    for line in toml_text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if inside {
                out.extend(missing_keys(&seen));
            }
            inside = t == header;
            seen.clear();
            out.push(line.to_string());
            continue;
        }
        if inside {
            if let Some((key, _)) = t.split_once('=') {
                let key = key.trim();
                if let Some((k, v)) = pairs.iter().find(|(k, _)| *k == key) {
                    out.push(format!("{k} = {v}"));
                    seen.push(k);
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if inside {
        out.extend(missing_keys(&seen));
    }

    let mut text = out.join("\n");
    if toml_text.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn write_validated(after: String) -> Result<(), String> {
    // Validate BEFORE writing: a text edit that produces invalid TOML
    // would leave the daemon without a config on the next boot.
    toml::from_str::<Config>(&after).map_err(|e| format!("a edicao quebrou o TOML: {e}"))?;
    let p = Config::path();
    std::fs::write(&p, after).map_err(|e| format!("write {p:?}: {e}"))
}

pub fn save_border_enabled(on: bool) -> Result<(), String> {
    let p = Config::path();
    let before = std::fs::read_to_string(&p).map_err(|e| format!("read {p:?}: {e}"))?;
    write_validated(set_in_section(&before, "input.border", &[("enabled", on.to_string())]))
}

/// Turns the reconciler on/off and picks who leads.
///
/// Configurable because the right answer depends on how the person
/// switches machines: with the border on, the mouse leads (it is what
/// crosses); switching with the keyboard's Easy-Switch button, the leader
/// has to be the KEYBOARD -- otherwise the reconciler pulls the keyboard
/// back to where the mouse is and undoes what the person just did.
pub fn save_follow(on: bool, leader: Option<&str>) -> Result<(), String> {
    let p = Config::path();
    let before = std::fs::read_to_string(&p).map_err(|e| format!("read {p:?}: {e}"))?;
    let mut pairs: Vec<(&str, String)> = vec![("enabled", on.to_string())];
    if let Some(l) = leader {
        pairs.push(("leader", format!("\"{}\"", l.replace('"', ""))));
    }
    write_validated(set_in_section(&before, "input.follow", &pairs))
}

/// Writes the positions to the file, preserving comments.
pub fn save_layout(pos: &BTreeMap<String, Cell>) -> Result<(), String> {
    let p = Config::path();
    let before = std::fs::read_to_string(&p).map_err(|e| format!("read {p:?}: {e}"))?;
    let after = apply_layout(&before, pos);
    // Validate BEFORE writing: a text edit that produces invalid TOML
    // would leave the daemon without a config on the next boot.
    toml::from_str::<Config>(&after).map_err(|e| format!("a edicao quebrou o TOML: {e}"))?;
    std::fs::write(&p, after).map_err(|e| format!("write {p:?}: {e}"))
}

/// Writes each host's grid position, preserving the file.
///
/// **Surgical text editing**, not serialization. Reserializing the struct
/// would produce correct TOML and throw away every comment -- and in this
/// config the comments are the record of why each choice was made (why
/// channel 3 is not mapped, why follow is off on Windows, why an IP is
/// never written). Losing that to save two coordinates would be a terrible
/// deal.
///
/// For each `[[hosts]]` with a known `name`: replaces the `layout` line if
/// there is one, otherwise inserts it right after `name`.
pub fn apply_layout(toml_text: &str, pos: &BTreeMap<String, Cell>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut current_host: Option<String> = None;
    let mut already_written = false;

    for line in toml_text.lines() {
        let t = line.trim();

        // Any table header closes the previous host.
        if t.starts_with("[[") || (t.starts_with('[') && !t.starts_with("[[")) {
            if t == "[[hosts]]" {
                current_host = None;
                already_written = false;
            } else if !t.starts_with("[hosts.") && !t.starts_with("[[hosts.") {
                current_host = None;
            }
            out.push(line.to_string());
            continue;
        }

        // Inside a [[hosts]], the `name` line identifies who it is.
        if current_host.is_none() {
            if let Some(name) = t
                .strip_prefix("name")
                .and_then(|r| r.trim_start().strip_prefix('='))
                .map(|r| r.trim().trim_matches('"').to_string())
            {
                if pos.contains_key(&name) {
                    current_host = Some(name.clone());
                    out.push(line.to_string());
                    let c = pos[&name];
                    out.push(format!("layout = {{ col = {}, row = {} }}", c.col, c.row));
                    already_written = true;
                    continue;
                }
            }
            out.push(line.to_string());
            continue;
        }

        // We already wrote the new one: the old one, if any, is dropped.
        if already_written && t.starts_with("layout") && t.contains('=') {
            continue;
        }
        out.push(line.to_string());
    }

    let mut text = out.join("\n");
    if toml_text.ends_with('\n') && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

impl Config {
    /// The local machine's neighbors on the grid: (edge, host name).
    ///
    /// "Immediately to the right" is the CLOSEST host with a larger `col`
    /// on the same row -- not `col + 1`. With a gap in the grid (three
    /// machines at 0, 1 and 3, for example) requiring exact adjacency
    /// would leave the edge without a target, and the person dragged the
    /// squares precisely to say who sits on which side.
    ///
    /// Hosts without `layout` are left out: a direction cannot be derived
    /// for something with no position.
    pub fn neighbors(&self) -> Vec<(&'static str, String)> {
        let Some(me) = self.hosts.iter().find(|h| h.is_local()) else { return Vec::new() };
        let Some(mine) = me.layout else { return Vec::new() };

        let mut v = Vec::new();
        for (edge, same_row, greater) in [
            ("right", true, true),
            ("left", true, false),
            ("bottom", false, true),
            ("top", false, false),
        ] {
            let mut cands: Vec<(i32, &HostCfg)> = self
                .hosts
                .iter()
                .filter(|h| !h.is_local())
                .filter_map(|h| h.layout.map(|c| (c, h)))
                .filter(|(c, _)| if same_row { c.row == mine.row } else { c.col == mine.col })
                .filter_map(|(c, h)| {
                    let d = if same_row { c.col - mine.col } else { c.row - mine.row };
                    let ok = if greater { d > 0 } else { d < 0 };
                    ok.then(|| (d.abs(), h))
                })
                .collect();
            cands.sort_by_key(|(d, _)| *d);
            if let Some((_, h)) = cands.first() {
                v.push((edge, h.name.clone()));
            }
        }
        v
    }

    /// The edges to watch: `(edge, target host)`.
    ///
    /// The grid rules when it exists. Without it, the hand-written
    /// `edge`/`target` pair applies -- configs from before the grid keep
    /// working, and whoever wants a single edge can still write it.
    pub fn border_edges(&self) -> Vec<(String, String)> {
        self.border_edges_full().into_iter().filter(|(_, _, ok)| *ok).map(|(e, t, _)| (e, t)).collect()
    }

    /// Like `border_edges`, but without hiding the discarded neighbors.
    ///
    /// The third field is "this target can RECEIVE". A host without `ssh`
    /// + `agent` has nobody to give the device back, so `focus::plan`
    /// blocks the switch -- creating a barrier for it would give an edge
    /// that can only refuse, filling the log every time the person touches
    /// it. It is a real case: the MacBook has no Logitech receiver.
    ///
    /// The UI shows both groups, so the grid does not silently lie about
    /// a neighbor that is there and does not work.
    pub fn border_edges_full(&self) -> Vec<(String, String, bool)> {
        let b = &self.input.border;
        if !b.enabled {
            return Vec::new();
        }
        let can_receive = |name: &str| {
            self.hosts
                .iter()
                .find(|h| h.name == name)
                .is_some_and(|h| h.ssh.is_some() && h.agent.is_some())
        };
        let from_grid = self.neighbors();
        if !from_grid.is_empty() {
            return from_grid
                .into_iter()
                .map(|(e, h)| {
                    let ok = can_receive(&h);
                    (e.to_string(), h, ok)
                })
                .collect();
        }
        match &b.target {
            Some(t) => {
                let ok = can_receive(t);
                vec![(b.edge.clone(), t.clone(), ok)]
            }
            None => Vec::new(),
        }
    }

    /// The first host that is not this machine.
    ///
    /// The caller is the Windows border's dead-man's switch, which needs
    /// to know where to give the device back. On Linux nobody calls it:
    /// there the guard lives on the target (`oxdesk-agent`), not the
    /// origin.
    ///
    /// With two hosts it is the peer, unambiguously. With more than two,
    /// the choice has to become explicit and the caller has to say which
    /// -- that is why it returns the first instead of guessing weights.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn peer_host(&self) -> Option<&HostCfg> {
        self.hosts.iter().find(|h| !h.is_local())
    }

    pub fn path() -> PathBuf {
        crate::sys::config_dir().join("config.toml")
    }

    /// Reads from disk; on the first run writes the default and uses it.
    pub fn load() -> Result<Self, String> {
        let p = Self::path();
        if !p.exists() {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
            }
            let initial = initial_config(DEFAULT_TOML, &local_hostname(), local_os());
            std::fs::write(&p, initial).map_err(|e| format!("write {p:?}: {e}"))?;
            gripe!("initial config written to {}", p.display());
        }
        let raw = std::fs::read_to_string(&p).map_err(|e| format!("read {p:?}: {e}"))?;
        let mut c: Config =
            toml::from_str(&raw).map_err(|e| format!("invalid TOML in {}: {e}", p.display()))?;
        // The screen kind from before the rename. legacy.rs rewrites the
        // file; this covers a config that could not be rewritten, so the
        // screen buttons do not vanish over a name.
        for h in &mut c.hosts {
            if let Some(s) = h.screen.as_mut().filter(|s| s.kind == "logistream") {
                s.kind = "oxdesk-stream".into();
            }
        }
        Ok(c)
    }

    /// Writes after validating: if the resulting TOML does not reopen, it
    /// does not write. Writes the whole config, reserialized.
    ///
    /// ⚠️ **Destroys every comment in the file.** Reserializing the struct
    /// produces correct TOML and loses everything that is not data -- and
    /// in this config the comments record why each choice was made.
    ///
    /// Today only `PATCH /api/config` calls this, and nothing in the UI
    /// uses that route. For targeted changes prefer surgical text editing,
    /// as `apply_layout` does.
    pub fn save(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| format!("serializar: {e}"))?;
        toml::from_str::<Config>(&text).map_err(|e| format!("self-validation failed: {e}"))?;
        let p = Self::path();
        std::fs::write(&p, text).map_err(|e| format!("write {p:?}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_describes_this_machine_and_nobody_else() {
        // `DEFAULT_TOML` is written on the first run AND by the wizard's
        // fix button -- so an error here only shows up on the new machine,
        // which is exactly where nobody can diagnose it.
        //
        // The assertion used to be "the default describes the three
        // machines", and it was right about the danger (a rewrite had
        // silently lost a whole machine) and wrong about the shape: those
        // three were OUR machines, and the first run of a stranger adopted
        // them. Step 4 of specs/018. What has to hold now is that the
        // default describes exactly one host, the local one, and names no
        // peer at all.
        let c: Config = toml::from_str(DEFAULT_TOML).expect("the default must parse");
        assert_eq!(c.hosts.len(), 1, "the default describes only this machine");
        assert_eq!(c.hosts.iter().filter(|h| h.is_local()).count(), 1, "exactly one local");
        assert!(c.devices.is_empty(), "channels are the user's hardware, not ours");
        assert!(c.input.border.target.is_none(), "the default names no peer");
        assert!(c.input.follow.leader.is_none(), "the default names no device");
        for h in &c.hosts {
            assert!(h.ssh.is_none(), "{}: no ssh alias in the default", h.name);
            assert!(h.agent.is_none(), "{}: no agent path in the default", h.name);
        }

        let mut names: Vec<&str> = c.hosts.iter().map(|h| h.name.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "duplicate host name");

        // Grid without collisions: two machines in the same cell make the
        // edge point at the wrong one.
        let mut cells: Vec<(i32, i32)> =
            c.hosts.iter().filter_map(|h| h.layout).map(|l| (l.col, l.row)).collect();
        // Against `hosts.len()` and not against a literal: the rule was
        // never "three", it was "all of them, each in its own cell". The
        // literal is what made this test fail for the right change.
        assert_eq!(cells.len(), c.hosts.len(), "every host has a grid position");
        cells.sort_unstable();
        cells.dedup();
        assert_eq!(cells.len(), c.hosts.len(), "two machines in the same grid cell");

        // No literal IP: it already broke everything once when the
        // Windows one changed.
        for h in &c.hosts {
            let ip = h.addr.split('.').count() == 4
                && h.addr.split('.').all(|p| p.parse::<u8>().is_ok());
            assert!(!ip || h.is_local(), "{} has a literal IP: {}", h.name, h.addr);
        }
    }

    #[test]
    fn expect_default_is_up() {
        // It matters: a service without a declared `expect` must alarm if
        // it goes down.
        assert_eq!(Expect::default(), Expect::Up);
    }

    #[test]
    fn minimal_toml_parses_and_applies_defaults() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
              [[hosts.services]]
              label = "SSH"
              port = 22
        "#;
        let c: Config = toml::from_str(t).expect("should parse");
        assert_eq!(c.port, 8787, "default port");
        assert_eq!(c.refresh_seconds, 10);
        assert_eq!(c.hosts.len(), 1);
        assert_eq!(c.hosts[0].services[0].expect, Expect::Up);
        assert!(c.devices.is_empty());
        assert!(!c.input.border.enabled, "border off by default");
    }

    #[test]
    fn expect_down_and_any_deserialize() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
              [[hosts.services]]
              label = "srv"
              port = 24800
              expect = "down"
              [[hosts.services]]
              label = "rdp"
              port = 3389
              expect = "any"
        "#;
        let c: Config = toml::from_str(t).unwrap();
        assert_eq!(c.hosts[0].services[0].expect, Expect::Down);
        assert_eq!(c.hosts[0].services[1].expect, Expect::Any);
    }

    #[test]
    fn who_am_i_by_loopback_or_by_flag() {
        // Heuristic: existing configs do not have the `local` key.
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "aqui"
            addr = "127.0.0.1"
            [[hosts]]
            name = "la"
            addr = "la.local"
            "#,
        )
        .unwrap();
        assert!(c.hosts[0].is_local());
        assert!(!c.hosts[1].is_local());
        assert_eq!(c.peer_host().map(|h| h.name.as_str()), Some("la"));

        // An explicit flag beats the address -- that is the Windows case,
        // where the local entry may not use loopback. A second, diverging
        // reading of this criterion once existed in `follow.rs` and was
        // removed; this test is what keeps it from coming back.
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "aqui"
            addr = "windows-pc.local"
            local = true
            [[hosts]]
            name = "la"
            addr = "127.0.0.1"
            local = false
            "#,
        )
        .unwrap();
        assert!(c.hosts[0].is_local(), "an explicit flag must beat the address");
        assert!(!c.hosts[1].is_local(), "local = false beats loopback");
        assert_eq!(c.peer_host().map(|h| h.name.as_str()), Some("la"));
    }

    #[test]
    fn saving_the_grid_keeps_comments() {
        let before = r#"# Config do oxdesk. Comentario que explica uma decisao cara.
port = 8787

[[hosts]]
name = "aqui"
addr = "127.0.0.1"
# Este comentario tem de sobreviver.
local = true

  [[hosts.services]]
  label = "SSH"
  port = 22

[[hosts]]
name = "la"
layout = { col = 9, row = 9 }
addr = "la.local"
"#;
        let mut pos = BTreeMap::new();
        pos.insert("aqui".to_string(), Cell { col: 0, row: 1 });
        pos.insert("la".to_string(), Cell { col: 2, row: 1 });
        let after = apply_layout(before, &pos);

        assert!(after.contains("# Comentario que explica uma decisao cara.")
                || after.contains("Comentario que explica uma decisao cara"));
        assert!(after.contains("# Este comentario tem de sobreviver."));
        assert!(after.contains("label = \"SSH\""), "subtable intact");
        // Inserted where there was none.
        assert!(after.contains("name = \"aqui\"\nlayout = { col = 0, row = 1 }"));
        // Replaced where there was one -- and the old one is gone.
        assert!(after.contains("layout = { col = 2, row = 1 }"));
        assert!(!after.contains("col = 9"), "the old position must go");
        // And it is still valid TOML with the rest in place.
        let c: Config = toml::from_str(&after).unwrap();
        assert_eq!(c.hosts.len(), 2);
        assert_eq!(c.hosts[0].services.len(), 1);
        assert_eq!(c.hosts[1].layout.map(|l| (l.col, l.row)), Some((2, 1)));
    }

    #[test]
    fn missing_key_is_appended() {
        // `leader` may never have been written in the section.
        let before = "[input.follow]\n# comentario\nenabled = false\n";
        let after = set_in_section(
            before,
            "input.follow",
            &[("enabled", "true".into()), ("leader", "\"MX Keys\"".into())],
        );
        assert!(after.contains("# comentario"));
        assert!(after.contains("enabled = true"));
        assert!(after.contains("leader = \"MX Keys\""));
    }

    #[test]
    fn border_toggle_preserves_the_section() {
        let before = "# top\n[input.border]\n# why it is like this\nenabled = false\nedge = \"right\"\n\n[input.follow]\nenabled = true\n";
        let after = set_in_section(before, "input.border", &[("enabled", "true".into())]);
        assert!(after.contains("# why it is like this"));
        assert!(after.contains("[input.border]\n# why it is like this\nenabled = true"));
        // Must not touch the OTHER block's enabled.
        assert!(after.contains("[input.follow]\nenabled = true"));
        let c: Config = toml::from_str(&after).unwrap();
        assert!(c.input.border.enabled);
        assert!(c.input.follow.enabled);

        // And turning it back off too.
        let c2: Config = toml::from_str(&set_in_section(&after, "input.border", &[("enabled", "false".into())])).unwrap();
        assert!(!c2.input.border.enabled);
        assert!(c2.input.follow.enabled, "follow must not be dragged along");
    }

    #[test]
    fn host_outside_the_map_stays_as_it_was() {
        let before = "[[hosts]]\nname = \"intocado\"\naddr = \"x.local\"\n";
        let after = apply_layout(before, &BTreeMap::new());
        assert_eq!(before, after);
    }

    #[test]
    fn neighbors_come_from_the_grid() {
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            layout = { col = 1, row = 1 }
            [[hosts]]
            name = "direita"
            addr = "d.local"
            layout = { col = 3, row = 1 }
            [[hosts]]
            name = "esquerda"
            addr = "e.local"
            layout = { col = 0, row = 1 }
            [[hosts]]
            name = "acima"
            addr = "a.local"
            layout = { col = 1, row = 0 }
            [[hosts]]
            name = "diagonal"
            addr = "x.local"
            layout = { col = 2, row = 2 }
            [[hosts]]
            name = "no position"
            addr = "s.local"
            "#,
        )
        .unwrap();
        let n = c.neighbors();
        let lookup = |b: &str| n.iter().find(|(e, _)| *e == b).map(|(_, h)| h.as_str());
        // Column 3 with a gap at 2: still the right-hand neighbor.
        assert_eq!(lookup("right"), Some("direita"));
        assert_eq!(lookup("left"), Some("esquerda"));
        assert_eq!(lookup("top"), Some("acima"));
        // A diagonal is not a neighbor on any edge.
        assert_eq!(lookup("bottom"), None);
        assert_eq!(n.len(), 3, "only the three edges with a neighbor");
    }

    #[test]
    fn the_closest_wins() {
        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            layout = { col = 0, row = 0 }
            [[hosts]]
            name = "longe"
            addr = "l.local"
            layout = { col = 5, row = 0 }
            [[hosts]]
            name = "perto"
            addr = "p.local"
            layout = { col = 1, row = 0 }
            "#,
        )
        .unwrap();
        assert_eq!(
            c.neighbors().iter().find(|(e, _)| *e == "right").map(|(_, h)| h.as_str()),
            Some("perto")
        );
    }

    #[test]
    fn no_grid_means_no_neighbor() {
        // Old config, without `layout`: the explicit edge still applies.

        let c: Config = toml::from_str(
            r#"
            [[hosts]]
            name = "eu"
            addr = "127.0.0.1"
            [[hosts]]
            name = "outro"
            addr = "o.local"
            "#,
        )
        .unwrap();
        assert!(c.neighbors().is_empty());
    }

    #[test]
    fn channel_map_by_device_name() {
        let t = r#"
            [[hosts]]
            name = "a"
            addr = "a.local"
            [[devices]]
            name = "MX Anywhere 3"
              [devices.hosts]
              "a" = 1
              "b" = 2
        "#;
        let c: Config = toml::from_str(t).unwrap();
        assert_eq!(c.devices[0].hosts.get("a"), Some(&1));
        assert_eq!(c.devices[0].hosts.get("b"), Some(&2));
    }
}

#[cfg(test)]
mod template_tests {
    use super::*;

    #[test]
    fn the_rendered_config_names_the_machine_it_was_written_on() {
        let out = initial_config(DEFAULT_TOML, "someones-laptop", "linux");
        let c: Config = toml::from_str(&out).expect("the rendered config must parse");
        assert_eq!(c.hosts[0].name, "someones-laptop");
        assert_eq!(c.hosts[0].os.as_deref(), Some("linux"));
        // No placeholder may survive: one left behind would be a host name
        // of literally "@NAME@", and the panel would key everything by it.
        assert!(!out.contains('@'), "a placeholder survived the substitution");
    }

    #[test]
    fn linux_watches_its_unit_and_the_others_watch_none() {
        let linux = initial_config(DEFAULT_TOML, "a", "linux");
        let c: Config = toml::from_str(&linux).unwrap();
        assert_eq!(c.hosts[0].units, vec!["oxdesk.service".to_string()]);

        for os in ["windows", "macos"] {
            let other = initial_config(DEFAULT_TOML, "a", os);
            let c: Config = toml::from_str(&other).unwrap();
            assert!(c.hosts[0].units.is_empty(), "{os} has no systemd units");
        }
    }

    #[test]
    fn a_machine_with_no_readable_name_gets_a_visible_placeholder() {
        // Better a name that is obviously a placeholder than `name = ""`,
        // which parses, looks fine and makes the local host unaddressable.
        for empty in ["", "   ", "\n"] {
            let out = initial_config(DEFAULT_TOML, empty, "linux");
            let c: Config = toml::from_str(&out).unwrap();
            assert_eq!(c.hosts[0].name, "this-machine");
        }
    }
}
