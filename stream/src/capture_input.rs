// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyboard and mouse capture through evdev, for the LAN KVM.
//!
//! Spec: ../specs/005-input-switch.md
//!
//! ## Why evdev and not Mutter's InputCapture
//!
//! GNOME 50 exposes `org.gnome.Mutter.InputCapture`, which is the API made
//! exactly for this. Introspecting the session, it offers `AddBarrier`,
//! `Enable`, `Disable`, `ConnectToEIS` and the signal
//! `Activated(barrier_id, ...)` -- and **no method to activate capture on
//! demand**. The compositor only hands over input when the pointer crosses
//! an edge barrier.
//!
//! That is the opposite of what was asked for: switching machines with a
//! click on the tray, without depending on an edge. It could be worked
//! around by creating a barrier and teleporting the pointer into it through
//! `RemoteDesktop` -- but that would be simulating the edge we are trying
//! to abandon, with more parts to break.
//!
//! evdev solves it directly:
//!
//! - **Grabbing and releasing is explicit** (`EVIOCGRAB`), which is the
//!   semantics of the tray click.
//! - **The codes are already ours.** `Event::Key` carries an evdev keycode
//!   because that is what Mutter's `NotifyKeyboardKeycode` receives, and
//!   `scancode_from_evdev()` already translates for Windows. There is no new
//!   conversion on the path.
//! - **Compositor-independent.** Same code on any Linux.
//!
//! ## What EVIOCGRAB does, and what it costs
//!
//! Grabbed, the device stops delivering to the compositor: the keyboard
//! simply does not exist for the local desktop. That is what we want when
//! sending input to another machine -- without the grab, every key would be
//! typed on BOTH computers.
//!
//! The cost is the responsibility: **if this process dies while grabbing,
//! the kernel releases** (the fd closes), so the keyboard cannot be locked
//! forever. But while it is grabbed there has to be a way back that does
//! not depend on the keyboard -- see `is_rescue`.
//!
//! ## Permission
//!
//! Reading `/dev/input/event*` requires an ACL. See
//! `oxdesk/udev/70-oxdesk-input.rules`: we use `TAG+="uaccess"` on the
//! Logitech devices, not the `input` group, because the group would give
//! any process of the account permanent access to every input device --
//! even with no session open.

#![cfg(target_os = "linux")]

use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::input::{Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

/// `_IOW('E', 0x90, int)`.
const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

// evdev event types we care about.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;

// Relative axes.
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;

// Mouse buttons, in evdev numbering.
const EV_BTN_LEFT: u16 = 0x110;
const EV_BTN_RIGHT: u16 = 0x111;
const EV_BTN_MIDDLE: u16 = 0x112;

/// One wheel notch, in the protocol's scroll pixels.
///
/// One notch is one line. It said 120 until 08/11 -- the number Windows uses
/// for its OWN wheel unit -- while the protocol's line is 15 px and the
/// injector on the other side divides by 15. The wheel therefore scrolled
/// eight lines per notch on Windows. See `input::SCROLL_PX_PER_LINE`.
const WHEEL_STEP: f64 = crate::input::SCROLL_PX_PER_LINE;

/// A 64-bit `struct input_event`: 2x i64 of time, u16, u16, i32.
const EVENT_SIZE: usize = 24;

/// Raw kernel event, already unpacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// Translates a batch of raw events (up to the `SYN_REPORT`) into ours.
///
/// Pure function: this is where the decision lives, and it is what the
/// tests look at.
///
/// evdev delivers movement on SEPARATE axes -- `REL_X` on one line,
/// `REL_Y` on another -- and only `SYN_REPORT` says the set is over.
/// Sending one network event per axis would double the traffic and make
/// the cursor move in a staircase on the other side, because each half
/// would arrive alone. That is why the axes add up within the batch and go
/// out as ONE `MouseRel`.
/// `repeat`: forward the kernel's auto-repeat as new key-downs. See the
/// note about `value == 2` inside the function.
pub fn translate(batch: &[RawEvent], repeat: bool) -> Vec<Event> {
    let mut out = Vec::new();
    let (mut dx, mut dy) = (0.0f64, 0.0f64);
    let (mut rx, mut ry) = (0.0f64, 0.0f64);

    for e in batch {
        match e.kind {
            EV_REL => match e.code {
                REL_X => dx += f64::from(e.value),
                REL_Y => dy += f64::from(e.value),
                // Inverted sign: in evdev, wheel UP is positive; in our
                // protocol (and in Wayland) down is positive, because the
                // content goes down.
                REL_WHEEL => ry -= f64::from(e.value) * WHEEL_STEP,
                REL_HWHEEL => rx += f64::from(e.value) * WHEEL_STEP,
                _ => {}
            },
            EV_KEY => {
                // value 2 = auto-repeat generated by the kernel.
                //
                // The first version discarded it, with the comment "the
                // other side repeats on its own from the key-down". That is
                // FALSE on Windows: key repeat there is born in the physical
                // keyboard's driver, and does not apply to a synthetic key
                // from `SendInput`. Result: holding backspace deleted a
                // single character -- that is how it showed up.
                //
                // Forwarding as key-down replicates exactly what the
                // physical keyboard does, which is a KVM's job.
                //
                // It sits behind `repeat` because a target that ALSO
                // repeats on its own (Mutter applies its own logic to
                // injected keys) would get doubled repeat. Whoever launches
                // the process knows the target's OS.
                if e.value == 2 && !repeat {
                    continue;
                }
                let pressed = e.value != 0;
                match e.code {
                    EV_BTN_LEFT => out.push(Event::Button { code: BTN_LEFT, pressed }),
                    EV_BTN_RIGHT => out.push(Event::Button { code: BTN_RIGHT, pressed }),
                    EV_BTN_MIDDLE => out.push(Event::Button { code: BTN_MIDDLE, pressed }),
                    code => out.push(Event::Key { keycode: u32::from(code), pressed }),
                }
            }
            _ => {}
        }
    }

    // Movement before the clicks of the SAME batch would be wrong for the
    // opposite reason: a click comes with the position where the batch
    // started. But within a batch the kernel does not mix click and
    // movement in a meaningful order, so the order here is stable: move,
    // then scroll. The buttons already went out in the order they arrived.
    if dx != 0.0 || dy != 0.0 {
        out.insert(0, Event::MouseRel { dx, dy });
    }
    if rx != 0.0 || ry != 0.0 {
        out.push(Event::Scroll { dx: rx, dy: ry });
    }
    out
}

/// An open `/dev/input/eventN`.
pub struct Device {
    file: File,
    pub name: String,
    pub path: PathBuf,
    grabbed: bool,
    batch: Vec<RawEvent>,
    /// Forward the kernel's auto-repeat. See `translate`.
    repeat: bool,
}

impl Device {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let name = name_of(path).unwrap_or_else(|| "?".into());
        Ok(Device {
            file,
            name,
            path: path.to_path_buf(),
            grabbed: false,
            batch: Vec::new(),
            repeat: true,
        })
    }

    /// Every `/dev/input/event*` we manage to open.
    ///
    /// The ones that deny access are SKIPPED silently on purpose: the
    /// machine has dozens of input nodes (power button, webcam, speaker)
    /// and none of them interests us. Complaining about permission is the
    /// caller's job (`oxdesk-kvm`), when a requested device is missing.
    pub fn list() -> Vec<Device> {
        let mut v = Vec::new();
        let Ok(dir) = std::fs::read_dir("/dev/input") else {
            return v;
        };
        let mut paths: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("event"))
            })
            .collect();
        paths.sort();
        for c in paths {
            if let Ok(d) = Device::open(&c) {
                v.push(d);
            }
        }
        v
    }

    /// The device stops existing for the local desktop.
    pub fn grab(&mut self) -> std::io::Result<()> {
        if self.grabbed {
            return Ok(());
        }
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGRAB, 1 as libc::c_int) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.grabbed = true;
        Ok(())
    }

    pub fn release(&mut self) -> std::io::Result<()> {
        if !self.grabbed {
            return Ok(());
        }
        let r = unsafe { libc::ioctl(self.file.as_raw_fd(), EVIOCGRAB, 0 as libc::c_int) };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        self.grabbed = false;
        Ok(())
    }

    /// Turns forwarding of the kernel's auto-repeat on or off.
    pub fn set_repeat(&mut self, r: bool) {
        self.repeat = r;
    }

    pub fn is_grabbed(&self) -> bool {
        self.grabbed
    }

    /// Reads whatever is there and returns the events of the COMPLETE batches.
    ///
    /// A batch only closes on `SYN_REPORT`. Events of a batch still open
    /// are kept for the next call -- delivering half a batch would send the
    /// X axis without the Y.
    pub fn read(&mut self) -> std::io::Result<Vec<Event>> {
        let mut buf = [0u8; EVENT_SIZE * 64];
        let n = self.file.read(&mut buf)?;
        let mut out = Vec::new();
        for chunk in buf[..n].chunks_exact(EVENT_SIZE) {
            let kind = u16::from_ne_bytes([chunk[16], chunk[17]]);
            let code = u16::from_ne_bytes([chunk[18], chunk[19]]);
            let value = i32::from_ne_bytes([chunk[20], chunk[21], chunk[22], chunk[23]]);
            if kind == EV_SYN {
                out.extend(translate(&self.batch, self.repeat));
                self.batch.clear();
            } else {
                self.batch.push(RawEvent { kind, code, value });
            }
        }
        Ok(out)
    }
}

impl Drop for Device {
    /// Releasing on `Drop` is a seat belt, not the mechanism.
    ///
    /// The kernel already releases the grab when the fd closes, so a
    /// keyboard locked forever is not a reachable state even if the process
    /// is killed with SIGKILL. This only makes the order explicit for
    /// whoever reads it.
    fn drop(&mut self) {
        let _ = self.release();
    }
}

/// Device name, from sysfs.
///
/// From sysfs and not from the `EVIOCGNAME` ioctl: the sysfs file can be
/// read without opening the input node, so we can list and choose BEFORE
/// having permission -- which is what allows saying "ACL missing on the MX
/// Keys" instead of "no device found".
pub fn name_of(path: &Path) -> Option<String> {
    let base = path.file_name()?.to_str()?;
    let p = format!("/sys/class/input/{base}/device/name");
    Some(std::fs::read_to_string(p).ok()?.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(code: u16, value: i32) -> RawEvent {
        RawEvent { kind: EV_REL, code, value }
    }
    fn key(code: u16, value: i32) -> RawEvent {
        RawEvent { kind: EV_KEY, code, value }
    }

    #[test]
    fn axes_of_the_same_batch_become_a_single_movement() {
        let out = translate(&[rel(REL_X, 5), rel(REL_Y, -3)], true);
        assert_eq!(out, vec![Event::MouseRel { dx: 5.0, dy: -3.0 }]);
    }

    #[test]
    fn batch_without_movement_sends_no_movement() {
        let out = translate(&[key(30, 1)], true);
        assert_eq!(out, vec![Event::Key { keycode: 30, pressed: true }]);
    }

    /// Holding a key has to delete several characters.
    ///
    /// Windows does not repeat a synthetic key -- repeat there is born in
    /// the physical keyboard's driver. Discarding the kernel's `value == 2`
    /// made holding backspace delete ONE character only, which is how the
    /// user found the bug.
    #[test]
    fn kernel_repeat_is_forwarded() {
        let out = translate(&[key(14, 1), key(14, 2), key(14, 2), key(14, 0)], true);
        assert_eq!(
            out,
            vec![
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: false },
            ]
        );
    }

    /// A target that repeats on its own gets only the start and the end.
    #[test]
    fn with_repeat_off_only_press_and_release_pass() {
        let out = translate(&[key(14, 1), key(14, 2), key(14, 2), key(14, 0)], false);
        assert_eq!(
            out,
            vec![
                Event::Key { keycode: 14, pressed: true },
                Event::Key { keycode: 14, pressed: false },
            ]
        );
    }

    #[test]
    fn buttons_become_buttons_and_not_keys() {
        let out = translate(&[key(EV_BTN_LEFT, 1), key(EV_BTN_RIGHT, 0)], true);
        assert_eq!(
            out,
            vec![
                Event::Button { code: BTN_LEFT, pressed: true },
                Event::Button { code: BTN_RIGHT, pressed: false },
            ]
        );
    }

    #[test]
    fn wheel_up_scrolls_up() {
        // In evdev, wheel up is +1. In our protocol (and in Wayland) the Y
        // axis grows downwards, so it has to come out NEGATIVE -- inverting
        // wrong makes the page move backwards, which is the kind of thing
        // you only find out by using it.
        let out = translate(&[rel(REL_WHEEL, 1)], true);
        assert_eq!(out, vec![Event::Scroll { dx: 0.0, dy: -WHEEL_STEP }]);
    }

    #[test]
    fn one_notch_is_one_line() {
        // The number and not the constant, on purpose: this is a wire unit,
        // and the test is here to break if someone moves it on one end only.
        let out = translate(&[rel(REL_WHEEL, 1)], true);
        assert_eq!(out, vec![Event::Scroll { dx: 0.0, dy: -15.0 }]);
    }

    #[test]
    fn movement_goes_out_before_the_batch_clicks() {
        let out = translate(&[key(EV_BTN_LEFT, 1), rel(REL_X, 2)], true);
        assert_eq!(out[0], Event::MouseRel { dx: 2.0, dy: 0.0 });
        assert_eq!(out[1], Event::Button { code: BTN_LEFT, pressed: true });
    }

    #[test]
    fn empty_batch_produces_nothing() {
        assert!(translate(&[], true).is_empty());
    }
}

// --- rescue ----------------------------------------------------------

/// evdev `KEY_LEFTCTRL` and `KEY_RIGHTCTRL`.
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_RIGHTCTRL: u32 = 97;

/// Both Ctrl keys together: release everything and go back to the local machine.
///
/// There has to be a way back that **does not depend on the network**.
/// Grabbed, the keyboard does not exist for the local desktop -- if the
/// target goes down, without this the person is left without a keyboard on
/// their own machine and the only way out is the mouse (which is grabbed
/// too) or another machine.
///
/// Both Ctrl keys because no program uses the combination and no text
/// produces it: it cannot be triggered by accident while typing. It is the
/// same gesture hardware KVM switches use, for the same reason.
pub fn is_rescue(pressed: &std::collections::HashSet<u32>) -> bool {
    pressed.contains(&KEY_LEFTCTRL) && pressed.contains(&KEY_RIGHTCTRL)
}

#[cfg(test)]
mod rescue_tests {
    use super::*;
    use std::collections::HashSet;

    fn set(v: &[u32]) -> HashSet<u32> {
        v.iter().copied().collect()
    }

    #[test]
    fn both_ctrls_ask_for_rescue() {
        assert!(is_rescue(&set(&[KEY_LEFTCTRL, KEY_RIGHTCTRL])));
    }

    #[test]
    fn a_single_ctrl_does_not() {
        assert!(!is_rescue(&set(&[KEY_LEFTCTRL])));
        assert!(!is_rescue(&set(&[KEY_RIGHTCTRL])));
    }

    #[test]
    fn ctrl_c_does_not() {
        // 46 = KEY_C. A common shortcut must not release the KVM.
        assert!(!is_rescue(&set(&[KEY_LEFTCTRL, 46])));
    }

    #[test]
    fn both_ctrls_with_more_keys_still_ask() {
        // A panicking person presses everything. Requiring exclusivity would
        // make the rescue fail exactly when it is needed.
        assert!(is_rescue(&set(&[KEY_LEFTCTRL, KEY_RIGHTCTRL, 42, 56])));
    }
}
