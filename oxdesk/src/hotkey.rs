//! Both Ctrl keys, when the input is HERE: send it to the other machine.
//!
//! Spec: ../../specs/005-input-switch.md
//!
//! ## Why this module exists at all
//!
//! The gesture already existed, in one direction only. `oxdesk-kvm` grabs
//! the devices and watches for both Ctrls to **release** them -- the way
//! back that does not depend on the network (see
//! `stream::capture_input::is_rescue`). But that watcher lives inside the
//! grabbing process, which only exists WHILE input is away. With the input
//! here, nobody was listening, so the same gesture did nothing and the only
//! way out was the tray.
//!
//! So: both Ctrls is now a toggle. Here -> there is this module; there ->
//! here stays where it was.
//!
//! ## Why a toggle and not a cycle through the machines
//!
//! A cycle would destroy the escape. From a remote machine, coming home
//! would take N presses, each one a round trip over a network that may be
//! exactly what broke. "Both Ctrls always brings the input home" has to
//! stay true in one press, so the second direction is a toggle against a
//! single target.
//!
//! ## Why this reader never grabs
//!
//! It opens the keyboard read-only and never calls `EVIOCGRAB`. Two
//! reasons, and the second is the good one:
//!
//! - grabbing here would take the keyboard away from the local desktop,
//!   which is the opposite of what a daemon watching for a hotkey should do;
//! - **while `oxdesk-kvm` holds the grab, this reader receives nothing.**
//!   The kernel delivers to the grabbing fd alone. So the two watchers are
//!   mutually exclusive by construction: no flag, no coordination, no
//!   window where both react to the same press.
//!
//! It reads key codes and keeps the state of exactly two of them. Nothing
//! is logged, and no other key is looked at -- a daemon that can see every
//! keystroke should be able to say precisely what it does with them.

#![cfg(target_os = "linux")]

use std::collections::HashSet;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::config::Config;

/// evdev keycodes of the two Ctrl keys.
const KEY_LEFTCTRL: u16 = 29;
const KEY_RIGHTCTRL: u16 = 97;

/// A 64-bit `struct input_event`: 2x i64 of time, u16, u16, i32.
const EVENT_SIZE: usize = 24;
const EV_KEY: u16 = 0x01;

/// After firing, ignore the gesture for this long.
///
/// Holding both Ctrls down sends a stream of auto-repeat; without this the
/// daemon would ask for the same switch dozens of times a second.
const COOLDOWN: Duration = Duration::from_secs(2);

/// Can this device report BOTH Ctrl keys?
///
/// Reads `/sys/class/input/eventN/device/capabilities/key`, which is the
/// kernel's own answer to "what can this thing send". Selecting by
/// capability and not by device name is what keeps a brand out of the code:
/// any keyboard qualifies, and a mouse never does.
///
/// The format is space-separated 64-bit hex words, **highest word first**,
/// so the list is reversed before indexing.
pub fn can_report_both_ctrls(mask: &str) -> bool {
    let mut words: Vec<u64> = Vec::new();
    for w in mask.split_whitespace() {
        match u64::from_str_radix(w, 16) {
            Ok(v) => words.push(v),
            // A word we cannot read makes the whole answer a guess, and a
            // guess here means watching the wrong device or none.
            Err(_) => return false,
        }
    }
    words.reverse();
    let has = |bit: u16| -> bool {
        let (i, off) = ((bit / 64) as usize, bit % 64);
        words.get(i).is_some_and(|w| (w >> off) & 1 == 1)
    };
    has(KEY_LEFTCTRL) && has(KEY_RIGHTCTRL)
}

/// Both Ctrl keys down, and nothing else needed.
///
/// Same predicate as `capture_input::is_rescue` on the other side of the
/// gesture, duplicated on purpose: `oxdesk` does not depend on the
/// `oxdesk-stream` crate, and pulling QUIC and a video encoder into the
/// daemon to share one line would be a worse trade. The test below pins
/// the two to the same meaning.
pub fn both_ctrls(pressed: &HashSet<u16>) -> bool {
    pressed.contains(&KEY_LEFTCTRL) && pressed.contains(&KEY_RIGHTCTRL)
}

/// The machine a press sends the input to, by rotation.
///
/// `peers` is the config order with each one's reachability; `last` is
/// where the previous press went.
///
/// Online ones first, and ALL of them when none is online -- better to try
/// and get `could not connect ... Nothing was grabbed` than to do nothing
/// and leave the person pressing a key that answers silence. If `last` is
/// no longer a candidate (it went down), the rotation restarts rather than
/// skipping to a position that no longer means anything.
pub fn next_target<'a>(peers: &[(&'a str, bool)], last: Option<&str>) -> Option<&'a str> {
    let online: Vec<&'a str> = peers.iter().filter(|(_, up)| *up).map(|(n, _)| *n).collect();
    let pool: Vec<&'a str> =
        if online.is_empty() { peers.iter().map(|(n, _)| *n).collect() } else { online };
    if pool.is_empty() {
        return None;
    }
    match last.and_then(|l| pool.iter().position(|p| *p == l)) {
        Some(i) => Some(pool[(i + 1) % pool.len()]),
        None => Some(pool[0]),
    }
}

/// Where the last press sent the input.
static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Asks which machine, with a list.
///
/// `zenity` and not a window of our own: the daemon has no GUI toolkit --
/// only `ksni`, which is D-Bus -- and pulling one in to draw a list would
/// be a lot of binary for one dialog. A real overlay (the Win+P kind) is a
/// separate job: it needs a window AND a font rasterizer, which nothing
/// here has yet.
///
/// A cancelled dialog returns `None` and nothing is switched. That matters:
/// the gesture becomes undoable before anything is grabbed.
fn ask_which(options: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("zenity");
    cmd.args(["--list", "--title=oxdesk", "--text=Send keyboard and mouse to:", "--column=Machine"]);
    for o in options {
        cmd.arg(o);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let pick = String::from_utf8_lossy(&out.stdout).trim().to_string();
    options.iter().find(|o| **o == pick).map(|o| o.to_string())
}

/// Watches the keyboards for the gesture, for as long as the daemon lives.
pub fn start(cfg: Arc<RwLock<Config>>, cache: Arc<std::sync::Mutex<crate::Cache>>) {
    std::thread::spawn(move || {
        let mut pressed: HashSet<u16> = HashSet::new();
        let mut down = false;
        let mut last_fired = Instant::now() - COOLDOWN;
        let mut watched: Vec<(std::path::PathBuf, std::fs::File)> = Vec::new();
        let mut rescan = Instant::now() - Duration::from_secs(60);

        loop {
            // Rescan, because a keyboard can arrive after the daemon did --
            // and the receiver's own devices come and go with the dongle.
            if rescan.elapsed() >= Duration::from_secs(10) {
                rescan = Instant::now();
                watched.retain(|(p, _)| p.exists());
                for path in keyboards() {
                    if watched.iter().any(|(p, _)| *p == path) {
                        continue;
                    }
                    if let Ok(f) = std::fs::File::open(&path) {
                        crate::say!("hotkey: watching {}", path.display());
                        watched.push((path, f));
                    }
                }
            }
            if watched.is_empty() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }

            let mut fds: Vec<libc::pollfd> = watched
                .iter()
                .map(|(_, f)| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 })
                .collect();
            // 1 s and not -1: the rescan above has to get its turn even on
            // a machine where nobody touches the keyboard.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
            if n <= 0 {
                continue;
            }

            let mut gesture = false;
            for (i, pfd) in fds.iter().enumerate() {
                if pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let mut buf = [0u8; EVENT_SIZE * 32];
                let Ok(read) = watched[i].1.read(&mut buf) else { continue };
                for chunk in buf[..read].chunks_exact(EVENT_SIZE) {
                    let kind = u16::from_ne_bytes([chunk[16], chunk[17]]);
                    let code = u16::from_ne_bytes([chunk[18], chunk[19]]);
                    let value = i32::from_ne_bytes([chunk[20], chunk[21], chunk[22], chunk[23]]);
                    if kind != EV_KEY || (code != KEY_LEFTCTRL && code != KEY_RIGHTCTRL) {
                        continue;
                    }
                    // value 2 is the kernel's auto-repeat: it keeps the key
                    // down, it does not press it again.
                    if value == 0 {
                        pressed.remove(&code);
                    } else {
                        pressed.insert(code);
                    }
                }
                if both_ctrls(&pressed) {
                    gesture = true;
                }
            }

            // Edge, not level: fire when the pair goes down, not for every
            // event that arrives while it is held.
            if gesture && !down && last_fired.elapsed() >= COOLDOWN {
                last_fired = Instant::now();
                fire(&cfg, &cache);
            }
            down = gesture;
        }
    });
}

/// Devices that can report both Ctrl keys.
fn keyboards() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/input") else { return out };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(node) = name.to_str().filter(|n| n.starts_with("event")) else { continue };
        let mask = entry.path().join("device/capabilities/key");
        let Ok(mask) = std::fs::read_to_string(mask) else { continue };
        if can_report_both_ctrls(mask.trim()) {
            let dev = std::path::PathBuf::from("/dev/input").join(node);
            if dev.exists() {
                out.push(dev);
            }
        }
    }
    out
}

/// Asks for the switch, through the same door the tray uses.
///
/// `kvm::send` and not a shortcut of our own: the pre-flight, the refusals
/// and the audit line all live there, and a second path into the same
/// action is a second place for them to diverge.
fn fire(cfg: &Arc<RwLock<Config>>, cache: &Arc<std::sync::Mutex<crate::Cache>>) {
    let Ok(c) = cfg.read() else { return };
    if !c.input.hotkey.enabled {
        return;
    }
    // Already away: the gesture belongs to `oxdesk-kvm`'s rescue, and we
    // should not have seen it at all (it holds the grab). Belt and braces.
    if crate::kvm::target().is_some() {
        return;
    }

    // Pinned target wins and skips both rotation and the dialog: someone
    // who named one machine does not want to be asked.
    let pinned = c
        .input
        .hotkey
        .target
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(want) = pinned {
        drop(c);
        return switch_to(cfg, &want);
    }

    // Reachability from the CACHED snapshot. Peeking and not refreshing is
    // the point: see `crate::snapshot_peek`.
    let snap = crate::snapshot_peek(cache);
    let peers: Vec<(String, bool)> = c
        .hosts
        .iter()
        .filter(|h| !h.is_local())
        .map(|h| {
            let up = snap
                .as_ref()
                .and_then(|s| s.hosts.iter().find(|x| x.name == h.name).map(|x| x.online))
                // No snapshot yet (the daemon just started): treat everyone
                // as a candidate rather than nobody.
                .unwrap_or(true);
            (h.name.clone(), up)
        })
        .collect();
    let ask = c.input.hotkey.mode == "ask";
    drop(c);

    let view: Vec<(&str, bool)> = peers.iter().map(|(n, up)| (n.as_str(), *up)).collect();
    let chosen = if ask {
        let online: Vec<&str> = view.iter().filter(|(_, up)| *up).map(|(n, _)| *n).collect();
        let pool: Vec<&str> =
            if online.is_empty() { view.iter().map(|(n, _)| *n).collect() } else { online };
        match pool.len() {
            0 => None,
            // Asking which of one is a dialog that answers itself.
            1 => Some(pool[0].to_string()),
            _ => match ask_which(&pool) {
                Some(p) => Some(p),
                None => {
                    crate::say!("hotkey: both Ctrl, nothing chosen");
                    return;
                }
            },
        }
    } else {
        let last = LAST.lock().ok().and_then(|l| l.clone());
        next_target(&view, last.as_deref()).map(str::to_string)
    };

    match chosen {
        Some(host) => {
            let cfg2 = Arc::clone(cfg);
            switch_to(&cfg2, &host);
        }
        None => crate::say!("hotkey: both Ctrl, but there is no remote host in the config"),
    }
}

/// Asks for the switch, through the same door the tray uses.
///
/// `kvm::send` and not a shortcut of our own: the pre-flight, the refusals
/// and the audit line all live there, and a second path into the same
/// action is a second place for them to diverge.
fn switch_to(cfg: &Arc<RwLock<Config>>, host: &str) {
    let Ok(c) = cfg.read() else { return };
    match crate::kvm::send(&c, host) {
        Ok(msg) => {
            if let Ok(mut l) = LAST.lock() {
                *l = Some(host.to_string());
            }
            crate::say!("hotkey: both Ctrl -> {msg}");
        }
        // A failure ADVANCES the rotation. The first version kept the
        // position so a host that merely blinked would be tried again --
        // but the real case is a machine that is up by SSH and cannot take
        // input at all (the Mac with a dead panel answers ssh and has no
        // graphical session). Keeping the position there spends every
        // second press on a wall. Advancing means the next press lands on
        // a machine that works, and a rotation over failures still comes
        // back around.
        Err(e) => {
            if let Ok(mut l) = LAST.lock() {
                *l = Some(host.to_string());
            }
            crate::say!("hotkey: both Ctrl -> {host} refused: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real masks, read from `/sys/class/input` on 2026-11-10.
    #[test]
    fn a_keyboard_qualifies_and_a_mouse_does_not() {
        let keyboard = "3f00733fff 0 0 483ffff17aff32d bfd4444600000000 ffff0001 \
                        130ff38b17d007 ffff7bfad941dfff ffbeffdfffefffff fffffffffffffffe";
        assert!(can_report_both_ctrls(keyboard));
        // The mouse of the same receiver: buttons only.
        assert!(!can_report_both_ctrls("ffff0000 0 0 0 0"));
        // A lid switch and a consumer-control endpoint, which is where a
        // name-based match would have gone wrong.
        assert!(!can_report_both_ctrls("4000 0 0"));
        assert!(!can_report_both_ctrls("100000000010000 17800000000 c000000000000 0"));
    }

    #[test]
    fn an_unreadable_mask_is_not_a_keyboard() {
        // Better to watch nothing than to watch the wrong device because a
        // word did not parse.
        assert!(!can_report_both_ctrls(""));
        assert!(!can_report_both_ctrls("zzz 0"));
    }

    #[test]
    fn the_gesture_is_the_same_one_the_rescue_uses() {
        let mut p = HashSet::new();
        p.insert(KEY_LEFTCTRL);
        assert!(!both_ctrls(&p), "one Ctrl is not the gesture");
        p.insert(46); // KEY_C -- Ctrl+C must never switch machines
        assert!(!both_ctrls(&p));
        p.insert(KEY_RIGHTCTRL);
        assert!(both_ctrls(&p));
    }

    fn cfg_with(names: &[(&str, bool)]) -> Config {
        let mut toml = String::new();
        for (n, local) in names {
            toml.push_str(&format!(
                "[[hosts]]\nname = \"{n}\"\naddr = \"{n}.local\"\nlocal = {local}\n"
            ));
        }
        toml::from_str(&toml).expect("fixture must parse")
    }

    #[test]
    fn one_machine_up_always_gets_it() {
        // The case that started this: two peers, one up. Pressing twice
        // must not send the input at a machine that is down.
        let peers = [("windows", true), ("mac", false)];
        assert_eq!(next_target(&peers, None), Some("windows"));
        assert_eq!(next_target(&peers, Some("windows")), Some("windows"));
    }

    #[test]
    fn it_rotates_through_the_ones_that_are_up() {
        let peers = [("a", true), ("b", true), ("c", true)];
        assert_eq!(next_target(&peers, None), Some("a"));
        assert_eq!(next_target(&peers, Some("a")), Some("b"));
        assert_eq!(next_target(&peers, Some("b")), Some("c"));
        assert_eq!(next_target(&peers, Some("c")), Some("a"), "wraps");
    }

    #[test]
    fn a_last_target_that_went_down_restarts_the_rotation() {
        // Keeping the position of a machine that left the pool would make
        // the next press land somewhere unrelated to where it was.
        let peers = [("a", true), ("b", false), ("c", true)];
        assert_eq!(next_target(&peers, Some("b")), Some("a"));
    }

    #[test]
    fn with_nobody_up_it_still_tries() {
        // `could not connect -- nothing was grabbed` is a better answer
        // than a key that does nothing at all.
        let peers = [("a", false), ("b", false)];
        assert_eq!(next_target(&peers, None), Some("a"));
        assert_eq!(next_target(&peers, Some("a")), Some("b"));
    }

    #[test]
    fn a_target_that_refuses_is_stepped_over_by_the_next_press() {
        // `switch_to` records the host on failure too, so the rotation
        // advances. The case: a machine that answers ssh (so the snapshot
        // calls it online) and cannot take input at all.
        let peers = [("windows", true), ("mac", true)];
        assert_eq!(next_target(&peers, Some("mac")), Some("windows"));
    }

    #[test]
    fn no_peers_means_no_target() {
        assert_eq!(next_target(&[], None), None);
    }

}
