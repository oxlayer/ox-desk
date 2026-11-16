// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Watching the keyboard without taking it: the daemon's half of the
//! shortcut on Windows (spec 023).
//!
//! A low-level keyboard hook sees every key and **passes every key on**: it
//! never returns non-zero, so nothing typed on this machine is lost to it.
//! The other half -- swallowing the input while it is on another machine --
//! is `oxdesk-kvm`'s hook (`stream/src/grab_win.rs`). The two cannot both
//! answer one press: Windows calls the most recently installed hook first,
//! the KVM installs its own after this one, and it does not pass the event
//! on. Same mutual exclusion the Linux reader gets from `EVIOCGRAB`.
//!
//! Windows removes a low-level hook whose callback is slow (about 300 ms),
//! without telling anyone. The callback given to `watch_keys` must only
//! record and signal; whatever takes time goes to another thread.

use std::sync::OnceLock;

/// One key event, as the hook reports it.
#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    /// Virtual key, left/right specific for the modifiers (`VK_LCONTROL`...).
    pub vk: u32,
    /// Hardware scancode. Above `0xFF` for keys Windows fakes, like the
    /// left Ctrl that AltGr sends ahead of itself.
    pub scan: u32,
    pub down: bool,
    /// Sent by `SendInput`, not typed: our own host injecting what another
    /// machine forwards, for one.
    pub injected: bool,
}

type Callback = Box<dyn Fn(KeyEvent) + Send + Sync>;
static CALLBACK: OnceLock<Callback> = OnceLock::new();

const WH_KEYBOARD_LL: i32 = 13;
const LLKHF_INJECTED: u32 = 0x10;
const LLKHF_UP: u32 = 0x80;

#[repr(C)]
struct KbdLlHookStruct {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    extra_info: usize,
}

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(
        id: i32,
        proc: unsafe extern "system" fn(i32, usize, isize) -> isize,
        module: isize,
        thread: u32,
    ) -> isize;
    fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetMessageW(msg: *mut u8, hwnd: isize, min: u32, max: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
    fn GetLastError() -> u32;
}

unsafe extern "system" fn hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 && lparam != 0 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam as *const KbdLlHookStruct) };
        if let Some(f) = CALLBACK.get() {
            f(KeyEvent {
                vk: k.vk_code,
                scan: k.scan_code,
                down: k.flags & LLKHF_UP == 0,
                injected: k.flags & LLKHF_INJECTED != 0,
            });
        }
    }
    // Always passed on: this hook watches, it never takes.
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

/// Starts watching the keyboard on a thread of its own, for as long as the
/// process lives. Returns once the hook is installed, or why it was not.
///
/// One watcher per process: a second call is refused rather than replacing
/// the first callback under a hook that is already running.
pub fn watch_keys(f: Callback) -> Result<(), String> {
    if CALLBACK.set(f).is_err() {
        return Err("the keyboard is already being watched".into());
    }
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("watch-keys".into())
        .spawn(move || {
            // SAFETY: the callback is a plain function valid for the life
            // of the process; the message loop below is what Windows calls
            // it from.
            let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, hook, GetModuleHandleW(std::ptr::null()), 0) };
            if h == 0 {
                let _ = tx.send(Err(format!("SetWindowsHookExW failed (error {})", unsafe { GetLastError() })));
                return;
            }
            let _ = tx.send(Ok(()));
            // MSG is 48 bytes on x64; a larger buffer costs nothing and
            // does not depend on the SDK's padding.
            let mut msg = [0u8; 64];
            // SAFETY: standard message loop on this thread.
            while unsafe { GetMessageW(msg.as_mut_ptr(), 0, 0, 0) } > 0 {}
        })
        .map_err(|e| format!("could not start the keyboard watcher: {e}"))?;
    rx.recv().map_err(|_| "the keyboard watcher died before reporting".to_string())?
}
