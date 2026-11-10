// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Applying input on Windows: `SendInput`.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Keys go by SCANCODE, not by virtual key
//!
//! The protocol carries an evdev keycode, which is the physical POSITION of
//! the key (see `input::evdev_from_winit`). Here it is translated to a Set 1
//! scancode and injected with `KEYEVENTF_SCANCODE` -- with `wVk = 0`,
//! Windows resolves the scancode through the layout of the focused window.
//! That is what keeps the promise: each machine applies its OWN layout,
//! like a real keyboard plugged into it. Sending a virtual key would let
//! the Mac's layout decide what shows up on Windows.
//!
//! The table itself lives in `input::scancode_from_evdev`, next to its
//! inverse and the tests that cover it.
//!
//! ## The mouse goes by normalized position on the VIRTUAL DESKTOP
//!
//! The client sends pixels in the space of what was captured -- ONE monitor.
//! Absolute `SendInput` wants 0..65535 over the WHOLE virtual desktop.
//! Without converting, on a two-monitor machine the cursor would go to the
//! wrong place by a factor equal to the ratio of the widths.

#![cfg(windows)]

use crate::input::{scancode_from_evdev, wheel_notches, Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VK_PAUSE, VIRTUAL_KEY,
};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

/// One wheel notch, in the unit `SendInput` wants.
///
/// Only Windows' own number lives here. The conversion from the protocol's
/// pixels is `input::wheel_notches`, next to the unit it converts from and
/// to the tests that pin it.
const WHEEL_DELTA: f64 = 120.0;

/// Applies the client's events on this machine.
///
/// It keeps state (the scroll remainder and the origin of the captured
/// monitor), which is why it is a struct and not a free function.
pub struct Injector {
    /// Corner of the captured monitor inside the virtual desktop.
    ///
    /// Shared and not copied: if the person rearranges the monitors during
    /// the session, the capture updates this and the cursor keeps landing in
    /// the right place. A copy would go stale silently, which is the worst
    /// way for this bug to show up.
    origin: std::sync::Arc<std::sync::Mutex<(i32, i32)>>,
    /// Fraction of a line left over from the last scroll event.
    rem_y: f64,
}

impl Injector {
    pub fn new(origin: std::sync::Arc<std::sync::Mutex<(i32, i32)>>) -> Self {
        Injector { origin, rem_y: 0.0 }
    }

    pub fn apply(&mut self, e: Event) {
        match e {
            Event::Mouse { x, y } => self.move_to(x, y),
            Event::MouseRel { dx, dy } => self.move_relative(dx, dy),
            Event::Button { code, pressed } => self.button(code, pressed),
            Event::Scroll { dx, dy } => self.scroll(dx, dy),
            Event::Key { keycode, pressed } => self.key(keycode, pressed),
        }
    }

    fn move_to(&self, x: f64, y: f64) {
        let (vx, vy, vw, vh) = virtual_screen();
        if vw <= 1 || vh <= 1 {
            return;
        }
        let Ok(origin) = self.origin.lock() else { return };
        // Stream pixel -> desktop pixel -> 0..65535 of the virtual desktop.
        let dx = origin.0 as f64 + x;
        let dy = origin.1 as f64 + y;
        let nx = ((dx - vx as f64) * 65535.0 / (vw - 1) as f64).round();
        let ny = ((dy - vy as f64) * 65535.0 / (vh - 1) as f64).round();
        send(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: nx.clamp(0.0, 65535.0) as i32,
                    dy: ny.clamp(0.0, 65535.0) as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }

    fn button(&self, code: i32, pressed: bool) {
        let flag: MOUSE_EVENT_FLAGS = match (code, pressed) {
            (BTN_LEFT, true) => MOUSEEVENTF_LEFTDOWN,
            (BTN_LEFT, false) => MOUSEEVENTF_LEFTUP,
            (BTN_RIGHT, true) => MOUSEEVENTF_RIGHTDOWN,
            (BTN_RIGHT, false) => MOUSEEVENTF_RIGHTUP,
            (BTN_MIDDLE, true) => MOUSEEVENTF_MIDDLEDOWN,
            (BTN_MIDDLE, false) => MOUSEEVENTF_MIDDLEUP,
            _ => return,
        };
        send(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }

    fn scroll(&mut self, dx: f64, dy: f64) {
        // Horizontal scroll is left out for now: it needs MOUSEEVENTF_HWHEEL
        // and few windows honor it. Dropped here instead of accumulated, so
        // that no remainder pretends to be on its way somewhere.
        let _ = dx;
        // Inverted sign: in our protocol a positive `dy` scrolls DOWN; on
        // Windows a positive wheel goes UP.
        let notches = wheel_notches(-dy, &mut self.rem_y);
        if notches == 0 {
            return;
        }
        let d = (f64::from(notches) * WHEEL_DELTA) as i32;
        send(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: d as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }

    fn key(&self, keycode: u32, pressed: bool) {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        let (vk, scan) = match scancode_from_evdev(keycode) {
            Some((scan, extended)) => {
                flags |= KEYEVENTF_SCANCODE;
                if extended {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                (VIRTUAL_KEY(0), scan)
            }
            // Pause has no one-byte scancode; it goes as a virtual key.
            None if keycode == 119 => (VK_PAUSE, 0),
            None => return,
        };
        if !pressed {
            flags |= KEYEVENTF_KEYUP;
        }
        send(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }
}

fn send(events: &[INPUT]) {
    let n = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != events.len() {
        // Happens when the focused window has higher privilege than ours
        // (UIPI): SendInput is refused as a whole, with no error.
        eprintln!("  SendInput refused ({n}/{}): elevated window in focus?", events.len());
    }
}

/// (x, y, width, height) of the virtual desktop -- covers multi-monitor,
/// unlike `SM_CXSCREEN`, which only sees the primary.
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

impl Injector {
    /// Relative movement, without going through a screen coordinate.
    ///
    /// The KVM sends this. `move_to()` cannot be reused: that one
    /// normalizes to 0..65535 over the virtual desktop, and converting
    /// relative to absolute would require keeping a virtual cursor of our
    /// own -- which would diverge from the real cursor as soon as anything
    /// else moved the pointer.
    ///
    /// Without `MOUSEEVENTF_ABSOLUTE`, Windows applies its own pointer
    /// acceleration, which is what the person expects to feel. A game that
    /// reads raw input gets the movement directly.
    pub fn move_relative(&self, dx: f64, dy: f64) {
        // Through `send()`, and not `SendInput` by hand: that one checks
        // the return value and shouts when UIPI refuses (elevated window in
        // focus). Calling it directly, as this function used to, swallowed
        // exactly the case we need to see in the log.
        send(&[INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: dx.round() as i32,
                    dy: dy.round() as i32,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }
}

// --- screen off -------------------------------------------------------
//
// The host would not start when the machine had its screen off:
// `DuplicateOutput` refused all five attempts and the process died with
// the message about a locked screen -- which was wrong. There was no
// lock: moving the mouse on the machine did not ask for a password. It was
// just power saving having turned the video output off, and with no output
// on there is nothing to duplicate.
//
// This only shows up in real use, because it is precisely when the machine
// has been left alone for a while that you want to reach it from afar.

/// Wakes the screen without pressing anything and without moving the pointer.
///
/// Two relative `MOUSEEVENTF_MOVE` in the SAME call: +1 and then -1.
/// The pointer ends up where it was, and the pair is atomic, so nobody can
/// click between the two.
///
/// It has to be real movement. The first version sent a single (0,0),
/// assuming the idle counter was enough; Windows discards the no-op and
/// the screen stayed off. That showed up by measuring: `DuplicateOutput`
/// passed, the client connected, and no frame came out -- because the video
/// output was still off.
///
/// Does NOT solve a truly locked session: there the secure desktop is in
/// front and DXGI does not capture it by design. Known limit.
pub fn wake_screen() {
    let step = |dx: i32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut pair = [step(1), step(-1)];
    unsafe { SendInput(&mut pair, std::mem::size_of::<INPUT>() as i32) };
}

/// While it exists, the screen does not turn off on its own.
///
/// Power saving turning the output off in the middle of a session brings
/// the duplication down (`DXGI_ERROR_ACCESS_LOST`), so waking at startup is
/// not enough: we have to hold it while we are serving. On `Drop` the
/// state goes back to normal, and Windows resumes the idle count from where
/// it was -- we do not keep the machine awake after the host exits.
pub struct ScreenAwake;

impl ScreenAwake {
    pub fn new() -> Self {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
        };
        ScreenAwake
    }
}

impl Drop for ScreenAwake {
    fn drop(&mut self) {
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

/// Ends any running screensaver. Returns how many it killed.
///
/// **This was the real cause of "I can't wake Windows up".**
///
/// The screensaver runs on its own desktop ("Screen-saver"), and DXGI
/// refuses to duplicate for a process that is not on the current input
/// desktop: `DuplicateOutput` returns `E_ACCESSDENIED`, the same code as a
/// locked screen and a half-installed driver. I diagnosed both wrong causes
/// before looking at the process list and finding `Bubbles.scr` running,
/// with `LogonUI` absent and the video adapter OK.
///
/// It cannot be dismissed through `SendInput`: injection goes to the
/// desktop of the calling THREAD, which is `Default` -- the screensaver, on
/// another desktop, never even hears about it. That is why `wake_screen()`
/// solves a screen that is off and does not solve this one.
///
/// `SystemParametersInfo(SPI_SETSCREENSAVEACTIVE, FALSE)` does not work on
/// its own either: it prevents the next one, it does not end the one
/// already on screen.
///
/// Terminating the process is what is left, and it is what remote access
/// tools do. The risk is low: a screensaver has no state to lose, and
/// Windows itself kills it on any mouse movement.
pub fn end_screensaver() -> usize {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let mut killed = 0usize;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return 0;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..end]);
                if name.to_ascii_lowercase().ends_with(".scr") {
                    if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, e.th32ProcessID) {
                        if TerminateProcess(h, 0).is_ok() {
                            println!("  screensaver terminated: {name}");
                            killed += 1;
                        }
                        let _ = CloseHandle(h);
                    }
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    killed
}
