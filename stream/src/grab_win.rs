// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Grabbing keyboard and mouse on Windows: the LAN KVM's source side.
//!
//! Spec: ../../specs/024-windows-kvm-source.md
//!
//! Linux grabs with `EVIOCGRAB` (`capture_input.rs`): the kernel hands the
//! device to one file descriptor and the desktop stops seeing it. Windows
//! has no such thing for a USB receiver, so this uses the mechanism every
//! software KVM on Windows uses: **low-level hooks** (`WH_KEYBOARD_LL`,
//! `WH_MOUSE_LL`). Each event reaches our callback before any window, and
//! returning a non-zero value from it swallows the event.
//!
//! ## Why the rescue lives in the hook thread
//!
//! The way back must not depend on the network (spec 005). The forwarding
//! loop awaits QUIC writes, and a stalled peer can hold it; if the rescue
//! were decided there, a dead network would also mean a dead way back --
//! with every key swallowed. So the hook callback itself watches the two
//! keys of the rescue chord, and when both are down it unhooks on the spot.
//! Input is local again before the loop even learns of it.
//!
//! ## Why mouse movement is read against a parked cursor
//!
//! A swallowed move does not move the cursor, and the callback still
//! reports where the cursor *would* have gone. Parking the cursor at a
//! fixed point and reading `pt - park` gives the movement of each event.
//! Two consequences worth knowing:
//!
//! - the value is after Windows' pointer ballistics ("Enhance pointer
//!   precision"), and the target applies its own acceleration on top;
//! - when the grab ends the cursor is put back where it was, moved off any
//!   screen edge, so a cursor restored onto the edge that sent the input
//!   away does not count as a new arrival there and send it away again.
//!
//! ## What a hook cannot do
//!
//! Ctrl+Alt+Del never reaches a hook: it is the secure attention sequence,
//! handled below user mode. And Windows silently removes a low-level hook
//! whose callback takes longer than `LowLevelHooksTimeout` (about 300 ms).
//! The callbacks here only translate and queue, so they stay far below it,
//! and if it ever happened the failure is the safe one: input goes back to
//! this machine.

#![cfg(windows)]

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, GetMessageW, GetSystemMetrics, PostQuitMessage,
    PostThreadMessageW, SetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
    LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN,
    SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use crate::input::{
    chord_held, evdev_from_windows_key, inset_from_edges, Event, BTN_EXTRA, BTN_LEFT, BTN_MIDDLE,
    BTN_RIGHT, BTN_SIDE, SCROLL_PX_PER_LINE,
};

/// `LLMHF_INJECTED`: the event came from `SendInput`, not from a device.
const LLMHF_INJECTED: u32 = 0x01;
/// Windows' wheel unit: one notch.
const WHEEL_DELTA: f64 = 120.0;
/// How far from an edge the cursor is put back when the grab ends.
const RESTORE_INSET: i32 = 60;

/// Set by the hook when the rescue chord is pressed.
static RESCUED: AtomicBool = AtomicBool::new(false);
/// Events the hooks let through because they were injected, not typed.
/// Printed on exit: it is the proof the hooks were called at all, which a
/// grab nobody touched cannot give.
static PASSED: AtomicUsize = AtomicUsize::new(0);

/// How many injected events the hooks passed through untouched.
pub fn passed_through() -> usize {
    PASSED.load(Ordering::SeqCst)
}

/// What the callbacks need. They are plain `extern "system"` functions with
/// no context argument, and Windows runs them on the thread that installed
/// them, so the state lives in that thread.
struct HookState {
    tx: UnboundedSender<Event>,
    rescue: (u32, u32),
    /// Forward key auto-repeat? No for a target that repeats on its own
    /// (Mutter does), or held keys would repeat twice.
    repeat: bool,
    pressed: HashSet<u32>,
    park: (i32, i32),
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// What the grab needs to know.
pub struct GrabOptions {
    pub rescue: (u32, u32),
    pub repeat: bool,
}

/// A live grab. Dropping it ends the grab: hooks removed, cursor put back.
pub struct Grab {
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
    restore: POINT,
}

/// The virtual desktop: `(left, top, width, height)`, all monitors.
pub fn virtual_rect() -> (i32, i32, i32, i32) {
    // SAFETY: plain metric reads.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Did the grab end because the person pressed the rescue chord?
pub fn rescued() -> bool {
    RESCUED.load(Ordering::SeqCst)
}

impl Grab {
    /// Installs the hooks and parks the cursor. Every event from then on
    /// goes to `tx` and nowhere else, until the rescue chord, `stop`, or
    /// the drop of this value.
    ///
    /// Returns only after both hooks are installed, so the caller can say
    /// "grabbed" truthfully -- or with the reason they were not.
    pub fn start(opts: GrabOptions, tx: UnboundedSender<Event>) -> Result<Grab, String> {
        let mut restore = POINT::default();
        // SAFETY: plain Win32 calls with valid pointers.
        unsafe {
            let _ = GetCursorPos(&mut restore);
        }
        let park = unsafe { (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2) };

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        let join = std::thread::Builder::new()
            .name("grab-hooks".into())
            .spawn(move || hook_thread(opts, tx, park, ready_tx))
            .map_err(|e| format!("could not start the hook thread: {e}"))?;
        let thread_id = ready_rx
            .recv()
            .map_err(|_| "the hook thread died before reporting".to_string())??;
        Ok(Grab { thread_id, join: Some(join), restore })
    }

    /// Ends the grab from another thread.
    pub fn stop(&self) {
        stop_thread(self.thread_id);
    }

    /// The hook thread's id, for a thread that has to end the grab without
    /// owning it (`stop_thread`).
    pub fn thread(&self) -> u32 {
        self.thread_id
    }

    /// Where the cursor was when the grab started, as fractions of the
    /// virtual desktop: the height it left at is the height it enters the
    /// other screen at.
    pub fn entry_ratio(&self) -> (f64, f64) {
        let (vx, vy, vw, vh) = virtual_rect();
        let f = |p: i32, o: i32, len: i32| (f64::from(p - o) / f64::from((len - 1).max(1))).clamp(0.0, 1.0);
        (f(self.restore.x, vx, vw), f(self.restore.y, vy, vh))
    }

    /// Where the cursor reappears when the grab ends, instead of where it
    /// was: the input came home by an edge, so the cursor comes in by the
    /// matching edge here. Still moved off the edge on drop.
    pub fn restore_at(&mut self, x: i32, y: i32) {
        self.restore = POINT { x, y };
    }
}

/// Ends the grab whose hook thread has this id. Idempotent: posting to a
/// thread that already left its loop does nothing.
pub fn stop_thread(thread_id: u32) {
    // SAFETY: posting a message to a thread id; no pointer crosses.
    unsafe {
        let _ = PostThreadMessageW(thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
}

impl Drop for Grab {
    fn drop(&mut self) {
        self.stop();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let (x, y) = inset_from_edges((self.restore.x, self.restore.y), virtual_rect(), RESTORE_INSET);
        unsafe {
            let _ = SetCursorPos(x, y);
        }
    }
}

fn hook_thread(
    opts: GrabOptions,
    tx: UnboundedSender<Event>,
    park: (i32, i32),
    ready: std::sync::mpsc::Sender<Result<u32, String>>,
) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(HookState {
            tx,
            rescue: opts.rescue,
            repeat: opts.repeat,
            pressed: HashSet::new(),
            park,
        })
    });

    // SAFETY: the callbacks are valid for the life of the process, and the
    // hooks are removed below before this thread exits.
    let hooks = unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("GetModuleHandleW: {e}")));
                return;
            }
        };
        let kb = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(hinst.into()), 0) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("keyboard hook refused: {e}")));
                return;
            }
        };
        let mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst.into()), 0) {
            Ok(h) => h,
            Err(e) => {
                let _ = UnhookWindowsHookEx(kb);
                let _ = ready.send(Err(format!("mouse hook refused: {e}")));
                return;
            }
        };
        // Park only once both hooks hold: a parked cursor with no hook
        // would just be a cursor that jumped.
        let _ = SetCursorPos(park.0, park.1);
        (kb, mouse)
    };
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    // The hooks are called from this loop: without it, no callback runs
    // and every event waits for the timeout before Windows gives up on us.
    let mut msg = MSG::default();
    // SAFETY: standard message loop on this thread.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        let _ = UnhookWindowsHookEx(hooks.0);
        let _ = UnhookWindowsHookEx(hooks.1);
    }
    // Dropping the state drops the sender: the forwarding loop sees the
    // channel close and knows the grab is over.
    STATE.with(|s| *s.borrow_mut() = None);
}

/// Queues an event; a closed channel means the forwarder is gone, and a
/// grab with nobody to forward to must end at once.
fn forward(st: &HookState, e: Event) {
    if st.tx.send(e).is_err() {
        // SAFETY: posting quit to the current thread.
        unsafe { PostQuitMessage(0) };
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
    let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    // Injected input is not the person's keyboard (it may be our own host
    // typing what a viewer sends): it passes through untouched.
    if k.flags.0 & LLKHF_INJECTED.0 != 0 {
        PASSED.fetch_add(1, Ordering::SeqCst);
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let down = k.flags.0 & LLKHF_UP.0 == 0;
    let extended = k.flags.0 & LLKHF_EXTENDED.0 != 0;
    let Some(keycode) = evdev_from_windows_key(k.vkCode, k.scanCode, extended) else {
        // A key with no evdev equivalent (or the fake Ctrl that AltGr
        // generates, scancode 0x21D) is swallowed and not sent.
        return LRESULT(1);
    };
    STATE.with(|s| {
        let mut g = s.borrow_mut();
        let Some(st) = g.as_mut() else { return };
        let repeat = down && st.pressed.contains(&keycode);
        if down {
            st.pressed.insert(keycode);
        } else {
            st.pressed.remove(&keycode);
        }
        // The rescue is decided HERE, before anything is queued, and it
        // ends the grab on this thread: see the module header.
        if chord_held(&st.pressed, st.rescue) {
            RESCUED.store(true, Ordering::SeqCst);
            // SAFETY: posting quit to the current thread.
            unsafe { PostQuitMessage(0) };
            return;
        }
        if repeat && !st.repeat {
            return;
        }
        forward(st, Event::Key { keycode, pressed: down });
    });
    LRESULT(1)
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    // SAFETY: for WH_MOUSE_LL, lparam points to an MSLLHOOKSTRUCT.
    let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    if m.flags & LLMHF_INJECTED != 0 {
        PASSED.fetch_add(1, Ordering::SeqCst);
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    // The high word of mouseData: wheel delta, or which X button.
    let high = (m.mouseData >> 16) as u16;
    STATE.with(|s| {
        let g = s.borrow();
        let Some(st) = g.as_ref() else { return };
        let button = |code: i32, pressed: bool| Some(Event::Button { code, pressed });
        let x_button = |pressed: bool| {
            // 1 = XBUTTON1 ("back"), 2 = XBUTTON2 ("forward").
            let code = if high == 2 { BTN_EXTRA } else { BTN_SIDE };
            Some(Event::Button { code, pressed })
        };
        let e = match wparam.0 as u32 {
            WM_MOUSEMOVE => {
                let (dx, dy) = (m.pt.x - st.park.0, m.pt.y - st.park.1);
                (dx != 0 || dy != 0).then_some(Event::MouseRel { dx: dx as f64, dy: dy as f64 })
            }
            WM_LBUTTONDOWN => button(BTN_LEFT, true),
            WM_LBUTTONUP => button(BTN_LEFT, false),
            WM_RBUTTONDOWN => button(BTN_RIGHT, true),
            WM_RBUTTONUP => button(BTN_RIGHT, false),
            WM_MBUTTONDOWN => button(BTN_MIDDLE, true),
            WM_MBUTTONUP => button(BTN_MIDDLE, false),
            WM_XBUTTONDOWN => x_button(true),
            WM_XBUTTONUP => x_button(false),
            // Windows counts the wheel up as positive; the protocol scrolls
            // DOWN on positive `dy`, as Wayland does.
            WM_MOUSEWHEEL => Some(Event::Scroll {
                dx: 0.0,
                dy: -(high as i16 as f64) / WHEEL_DELTA * SCROLL_PX_PER_LINE,
            }),
            WM_MOUSEHWHEEL => Some(Event::Scroll {
                dx: (high as i16 as f64) / WHEEL_DELTA * SCROLL_PX_PER_LINE,
                dy: 0.0,
            }),
            _ => None,
        };
        if let Some(e) = e {
            forward(st, e);
        }
    });
    LRESULT(1)
}
