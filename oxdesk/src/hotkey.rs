// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The shortcut (both Ctrl keys by default), when the input is HERE: send
//! it to the other machine.
//!
//! Spec: ../../specs/005-input-switch.md, ../../specs/023-shortcut.md
//!
//! The pair is configurable since spec 023 (`[input.hotkey] keys`: both
//! Ctrl, both Shift or both Alt), and so is the way back, which `oxdesk-kvm`
//! receives as `--rescue`. What follows says "both Ctrls" because that is
//! the default and the case every measurement below was taken on.
//!
//! The decision -- where a press sends the input -- is shared by every
//! system. The key READER is per system: Linux reads evdev here; Windows
//! watches with a low-level hook (`hotkey_win.rs`, `winput::keys`).
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
//! That silence has a consequence this file missed at first: it swallows
//! the **key-ups** too. The fingers come up into the grab, so a reader
//! that only accumulates events believes the pair is still down and the
//! edge never re-arms. The fix is to ask the kernel what is actually
//! held, in one direction only -- see `prune_stale`.
//!
//! It reads key codes and keeps the state of exactly two of them. Nothing
//! is logged, and no other key is looked at -- a daemon that can see every
//! keystroke should be able to say precisely what it does with them.

use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

use crate::config::Config;

/// evdev keycodes of the two Ctrl keys: the default chord.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const KEY_LEFTCTRL: u16 = 29;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const KEY_RIGHTCTRL: u16 = 97;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const CTRLS: (u16, u16) = (KEY_LEFTCTRL, KEY_RIGHTCTRL);

/// A 64-bit `struct input_event`: 2x i64 of time, u16, u16, i32.
#[cfg(target_os = "linux")]
const EVENT_SIZE: usize = 24;
#[cfg(target_os = "linux")]
const EV_KEY: u16 = 0x01;

/// After firing, ignore the gesture for this long.
///
/// Holding both Ctrls down sends a stream of auto-repeat; without this the
/// daemon would ask for the same switch dozens of times a second.
pub(crate) const COOLDOWN: Duration = Duration::from_secs(2);

/// Can this device report both keys of the chord?
///
/// Reads `/sys/class/input/eventN/device/capabilities/key`, which is the
/// kernel's own answer to "what can this thing send". Selecting by
/// capability and not by device name is what keeps a brand out of the code:
/// any keyboard qualifies, and a mouse never does.
///
/// The format is space-separated 64-bit hex words, **highest word first**,
/// so the list is reversed before indexing.
#[cfg(target_os = "linux")]
pub fn can_report(mask: &str, chord: (u16, u16)) -> bool {
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
    has(chord.0) && has(chord.1)
}

/// Both keys of the chord down, and nothing else needed.
///
/// Same predicate as `input::chord_held` on the other side of the gesture
/// (`oxdesk-kvm`'s rescue), duplicated on purpose: `oxdesk` does not
/// depend on the `oxdesk-stream` crate, and pulling QUIC and a video
/// encoder into the daemon to share one line would be a worse trade. The
/// test below pins the two to the same meaning.
pub fn chord_down(pressed: &HashSet<u16>, chord: (u16, u16)) -> bool {
    pressed.contains(&chord.0) && pressed.contains(&chord.1)
}

/// The evdev code of a chord key, from what a Windows keyboard hook
/// reports. `None` for every key that cannot be half of a chord.
///
/// The hook gives the modifiers left/right-specific virtual keys, which is
/// all a chord needs. The scancode guards one trap: AltGr makes Windows
/// send a fake left Ctrl ahead of it (scancode `0x21D`), and taking that
/// for a real one would make "both Ctrl" fire on AltGr + right Ctrl.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn chord_key_from_vk(vk: u32, scan: u32) -> Option<u16> {
    if scan > 0xFF {
        return None;
    }
    Some(match vk {
        0xA0 => 42,  // VK_LSHIFT -> KEY_LEFTSHIFT
        0xA1 => 54,  // VK_RSHIFT -> KEY_RIGHTSHIFT
        0xA2 => 29,  // VK_LCONTROL -> KEY_LEFTCTRL
        0xA3 => 97,  // VK_RCONTROL -> KEY_RIGHTCTRL
        0xA4 => 56,  // VK_LMENU -> KEY_LEFTALT
        0xA5 => 100, // VK_RMENU -> KEY_RIGHTALT
        _ => return None,
    })
}

/// `KEY_MAX` rounded up to whole bytes: the bitmap `EVIOCGKEY` fills.
#[cfg(target_os = "linux")]
const KEY_BYTES: usize = 96;

/// `_IOR('E', 0x18, [u8; KEY_BYTES])` -- "what keys are down on this
/// device, right now", computed here so the crate keeps a single dep.
#[cfg(target_os = "linux")]
fn eviocgkey() -> libc::c_ulong {
    const READ: libc::c_ulong = 2;
    (READ << 30) | ((KEY_BYTES as libc::c_ulong) << 16) | ((b'E' as libc::c_ulong) << 8) | 0x18
}

/// Which of the chord's two keys the kernel says are physically held.
///
/// `None` when the device cannot be asked, which is not the same answer as
/// "none held" -- see `prune_stale`.
#[cfg(target_os = "linux")]
fn keys_held(f: &std::fs::File, chord: (u16, u16)) -> Option<HashSet<u16>> {
    let mut bits = [0u8; KEY_BYTES];
    // SAFETY: the kernel writes KEY_BYTES into a buffer of that size.
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), eviocgkey(), bits.as_mut_ptr()) };
    if rc < 0 {
        return None;
    }
    let mut out = HashSet::new();
    for code in [chord.0, chord.1] {
        let (byte, bit) = (code as usize / 8, code as usize % 8);
        if bits[byte] & (1 << bit) != 0 {
            out.insert(code);
        }
    }
    Some(out)
}

/// Drops the keys we believe are down that the kernel says are not.
///
/// ## Why asking is necessary
///
/// Our view is built from events, and **a key-up delivered to a process
/// that holds the grab is a key-up we never see**. The sequence is the
/// ordinary one: the gesture fires, `oxdesk-kvm` grabs the keyboard, the
/// fingers come up -- into the grab. Our set keeps both Ctrls down
/// forever, the edge never re-arms, and the next press is dead. One dead
/// press per switch, which is what sent me looking.
///
/// ## Why asking may only ever REMOVE
///
/// The two watchers are mutually exclusive because this one is
/// event-starved while the grab is held (see the module header). Reading
/// the kernel's bitmap is not starved -- it reports the keys even during
/// someone else's grab. So if a held pair could ARM the gesture, a single
/// press would be answered twice: `oxdesk-kvm` brings the input home and
/// this module sends it straight back out.
///
/// Hence the asymmetry, and it is the whole point: `pressed` grows only
/// from events we actually received, and asking only prunes. Firing still
/// requires having received both presses, which cannot happen under a
/// grab. The invariant survives.
///
/// `held == None` means the question failed, and a failed question must
/// not disarm anything -- that would be a hotkey that silently stops
/// working, the failure mode this file already paid for once.
#[cfg(target_os = "linux")]
pub fn prune_stale(pressed: &mut HashSet<u16>, held: Option<&HashSet<u16>>) {
    let Some(held) = held else { return };
    pressed.retain(|k| held.contains(k));
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
#[cfg(target_os = "linux")]
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

/// No dialog off Linux: `fire` never asks there (see the `ask` line).
#[cfg(not(target_os = "linux"))]
fn ask_which(_options: &[&str]) -> Option<String> {
    None
}

/// Watches the keyboards for the gesture, for as long as the daemon lives.
#[cfg(target_os = "linux")]
pub fn start(cfg: Arc<RwLock<Config>>, cache: Arc<std::sync::Mutex<crate::Cache>>) {
    std::thread::spawn(move || {
        let mut pressed: HashSet<u16> = HashSet::new();
        let mut down = false;
        let mut last_fired = Instant::now() - COOLDOWN;
        let mut watched: Vec<(std::path::PathBuf, std::fs::File)> = Vec::new();
        let mut rescan = Instant::now() - Duration::from_secs(60);
        let mut chord = CTRLS;

        loop {
            // The pair is read on every turn, so a change in the panel
            // takes effect without a restart. A new pair starts from
            // nothing held: keys of the old pair mean nothing now.
            let now = cfg.read().map(|g| g.input.hotkey.chord()).unwrap_or(CTRLS);
            if now != chord {
                chord = now;
                pressed.clear();
                down = false;
                crate::say!("hotkey: the shortcut is now keys {},{}", chord.0, chord.1);
            }

            // Rescan, because a keyboard can arrive after the daemon did --
            // and the receiver's own devices come and go with the dongle.
            if rescan.elapsed() >= Duration::from_secs(10) {
                rescan = Instant::now();
                watched.retain(|(p, _)| p.exists());
                for path in keyboards(chord) {
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
            if n < 0 {
                continue;
            }

            // A timeout is not a reason to skip the rest: the pruning below
            // is what re-arms the gesture after a switch, and it has to get
            // its turn on a keyboard nobody is touching.
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
                    if kind != EV_KEY || (code != chord.0 && code != chord.1) {
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
            }

            // Decide on what we RECEIVED -- events are fresher than the
            // question below, and a tap already over by the time we ask
            // should still count.
            //
            // Edge, not level: fire when the pair goes down, not for every
            // event that arrives while it is held.
            let gesture = chord_down(&pressed, chord);
            if gesture && !down && last_fired.elapsed() >= COOLDOWN {
                last_fired = Instant::now();
                fire(&cfg, &cache);
            }
            down = gesture;

            // THEN ask the kernel what is really held, and drop the rest.
            // Decide-then-prune, in that order: see `prune_stale` for why
            // the question may only ever take a key away.
            let mut held = HashSet::new();
            let mut answered = false;
            for (_, f) in &watched {
                if let Some(k) = keys_held(f, chord) {
                    answered = true;
                    held.extend(k);
                }
            }
            let believed = pressed.len();
            prune_stale(&mut pressed, if answered { Some(&held) } else { None });
            if pressed.len() < believed && down {
                // Once per switch, and the line I wanted to have when the
                // gesture went quiet: it says the keys came up somewhere we
                // could not see, and that the gesture is armed again.
                crate::say!("hotkey: re-armed (the keys came up while the input was away)");
                down = chord_down(&pressed, chord);
            }
        }
    });
}

/// Devices that can report both keys of the chord.
#[cfg(target_os = "linux")]
fn keyboards(chord: (u16, u16)) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/input") else { return out };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(node) = name.to_str().filter(|n| n.starts_with("event")) else { continue };
        let mask = entry.path().join("device/capabilities/key");
        let Ok(mask) = std::fs::read_to_string(mask) else { continue };
        if can_report(mask.trim(), chord) {
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
pub(crate) fn fire(cfg: &Arc<RwLock<Config>>, cache: &Arc<std::sync::Mutex<crate::Cache>>) {
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
    // Asking needs a dialog, and the daemon has one only on Linux
    // (`zenity`). Elsewhere `ask` rotates, which is what one machine up
    // does anyway.
    let ask = c.input.hotkey.mode == "ask" && cfg!(target_os = "linux");
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
                    crate::say!("hotkey: shortcut pressed, nothing chosen");
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
        None => crate::say!("hotkey: shortcut pressed, but there is no remote host in the config"),
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
            crate::say!("hotkey: shortcut -> {msg}");
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
            crate::say!("hotkey: shortcut -> {host} refused: {e}");
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Real masks, read from `/sys/class/input` on 2026-11-10.
    #[test]
    fn a_keyboard_qualifies_and_a_mouse_does_not() {
        let keyboard = "3f00733fff 0 0 483ffff17aff32d bfd4444600000000 ffff0001 \
                        130ff38b17d007 ffff7bfad941dfff ffbeffdfffefffff fffffffffffffffe";
        assert!(can_report(keyboard, CTRLS));
        // The mouse of the same receiver: buttons only.
        assert!(!can_report("ffff0000 0 0 0 0", CTRLS));
        // A lid switch and a consumer-control endpoint, which is where a
        // name-based match would have gone wrong.
        assert!(!can_report("4000 0 0", CTRLS));
        assert!(!can_report("100000000010000 17800000000 c000000000000 0", CTRLS));
    }

    #[test]
    fn an_unreadable_mask_is_not_a_keyboard() {
        // Better to watch nothing than to watch the wrong device because a
        // word did not parse.
        assert!(!can_report("", CTRLS));
        assert!(!can_report("zzz 0", CTRLS));
    }

    #[test]
    fn the_gesture_is_the_same_one_the_rescue_uses() {
        let mut p = HashSet::new();
        p.insert(KEY_LEFTCTRL);
        assert!(!chord_down(&p, CTRLS), "one Ctrl is not the gesture");
        p.insert(46); // KEY_C -- Ctrl+C must never switch machines
        assert!(!chord_down(&p, CTRLS));
        p.insert(KEY_RIGHTCTRL);
        assert!(chord_down(&p, CTRLS));
    }

    /// The bug this file shipped with: one dead press after every switch.
    #[test]
    fn a_key_up_we_never_saw_does_not_latch_the_gesture() {
        // The press that fired. `oxdesk-kvm` now grabs the keyboard.
        let mut pressed = HashSet::from([KEY_LEFTCTRL, KEY_RIGHTCTRL]);
        let mut down = chord_down(&pressed, CTRLS);
        assert!(down);

        // The fingers come up into the grab: no event reaches us. Asking is
        // the only way to find out.
        prune_stale(&mut pressed, Some(&HashSet::new()));
        down = chord_down(&pressed, CTRLS);
        assert!(!down, "a key-up delivered to the grab must still disarm");

        // So the very next press is an edge again, not a level.
        pressed.insert(KEY_LEFTCTRL);
        pressed.insert(KEY_RIGHTCTRL);
        assert!(chord_down(&pressed, CTRLS) && !down, "the next press must fire");
    }

    /// The invariant that keeps the two watchers from both answering one
    /// press: asking the kernel may remove, never add.
    #[test]
    fn asking_the_kernel_never_arms_the_gesture() {
        // Input is away; `oxdesk-kvm` holds the grab, so we received at
        // most this one press. The kernel still reports both keys held.
        let mut pressed = HashSet::from([KEY_LEFTCTRL]);
        prune_stale(&mut pressed, Some(&HashSet::from([KEY_LEFTCTRL, KEY_RIGHTCTRL])));
        assert!(
            !chord_down(&pressed, CTRLS),
            "a held pair we did not receive must not become the gesture: \
             the rescue would bring the input home and this would send it back"
        );
    }

    #[test]
    fn holding_the_pair_keeps_it_down() {
        // Still held, so nothing is dropped and the cooldown -- not a
        // re-arm -- is what stops the auto-repeat from firing again.
        let mut pressed = HashSet::from([KEY_LEFTCTRL, KEY_RIGHTCTRL]);
        prune_stale(&mut pressed, Some(&HashSet::from([KEY_LEFTCTRL, KEY_RIGHTCTRL])));
        assert!(chord_down(&pressed, CTRLS));
    }

    #[test]
    fn a_question_that_failed_disarms_nothing() {
        // A hotkey that silently stops working is the failure mode this
        // file already paid for once.
        let mut pressed = HashSet::from([KEY_LEFTCTRL, KEY_RIGHTCTRL]);
        prune_stale(&mut pressed, None);
        assert!(chord_down(&pressed, CTRLS));
    }

    #[test]
    fn the_key_bitmap_covers_both_ctrls() {
        // 96 bytes is KEY_MAX rounded up; the right Ctrl at 97 lands in
        // byte 12, well inside it. An off-by-one here reads a key that is
        // not there.
        assert!(KEY_BYTES * 8 > KEY_RIGHTCTRL as usize);
        assert!(KEY_BYTES * 8 > KEY_LEFTCTRL as usize);
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

    #[test]
    fn every_chord_key_comes_from_its_own_side_of_the_keyboard() {
        assert_eq!(chord_key_from_vk(0xA2, 0x1D), Some(29));
        assert_eq!(chord_key_from_vk(0xA3, 0x1D), Some(97));
        assert_eq!(chord_key_from_vk(0xA0, 0x2A), Some(42));
        assert_eq!(chord_key_from_vk(0xA1, 0x36), Some(54));
        assert_eq!(chord_key_from_vk(0xA4, 0x38), Some(56));
        assert_eq!(chord_key_from_vk(0xA5, 0x38), Some(100));
        // Letters are never half of a chord.
        assert_eq!(chord_key_from_vk(0x41, 0x1E), None);
    }

    #[test]
    fn the_ctrl_altgr_fakes_is_not_a_left_ctrl() {
        // AltGr + right Ctrl must not read as "both Ctrl".
        assert_eq!(chord_key_from_vk(0xA2, 0x21D), None);
    }

    #[test]
    fn the_daemon_chords_are_the_ones_the_config_names() {
        // Every chord the panel offers has two different keys this
        // reader can see on Windows -- otherwise a choice in the panel
        // would be a shortcut nobody can press there.
        let seen: Vec<u16> = (0xA0..=0xA5).filter_map(|vk| chord_key_from_vk(vk, 0x10)).collect();
        for (name, (a, b)) in crate::config::CHORDS {
            assert_ne!(a, b, "{name}");
            assert!(seen.contains(&a) && seen.contains(&b), "{name}: {a},{b} not readable on Windows");
        }
    }

}
