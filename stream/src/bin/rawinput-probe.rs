// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Does a low-level mouse hook that swallows an event also hide it from
//! Raw Input? Spec 025 section 2 is built on the answer, so it is measured
//! before anything is built on it.
//!
//! The probe touches nobody's mouse. It injects `N` moves of +1/-1 px with
//! a marker in `dwExtraInfo`, its hook swallows only the moves carrying the
//! marker (everything else passes), and a message-only window registered
//! for Raw Input with `RIDEV_INPUTSINK` counts the marked moves that still
//! reach it (`RAWMOUSE::ulExtraInformation`).
//!
//!   oxdesk-probe-rawinput [N]
//!
//! It prints three counts: injected, swallowed by the hook, seen by Raw
//! Input. A fourth run with the hook off is the control: Raw Input must see
//! all of them there, or the probe measures nothing.
//!
//! What it does not answer: whether a real device behaves like `SendInput`.
//! That needs a person moving the mouse; the probe says how.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    let n: usize = std::env::args().nth(1).and_then(|v| v.parse().ok()).unwrap_or(50);
    for hook in [false, true] {
        let r = probe::run(n, hook);
        println!(
            "hook {}: injected {}, swallowed by the hook {}, seen by Raw Input {}",
            if hook { "ON " } else { "OFF" },
            r.injected,
            r.swallowed,
            r.raw_seen
        );
    }
}

#[cfg(windows)]
mod probe {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    use windows::Win32::UI::Input::{
        GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE,
        RAWINPUTHEADER, RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEMOUSE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        PeekMessageW, RegisterClassW, SetWindowsHookExW, UnhookWindowsHookEx, HWND_MESSAGE,
        MSG, MSLLHOOKSTRUCT, PM_REMOVE, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_INPUT, WNDCLASSW,
    };

    /// The marker in `dwExtraInfo`: the probe's own moves, and only those.
    const MARK: usize = 0x0D35_0025;

    static SWALLOWED: AtomicUsize = AtomicUsize::new(0);
    static RAW_SEEN: AtomicUsize = AtomicUsize::new(0);

    pub struct Counts {
        pub injected: usize,
        pub swallowed: usize,
        pub raw_seen: usize,
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            // SAFETY: for WH_MOUSE_LL, lparam points to an MSLLHOOKSTRUCT.
            let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if m.dwExtraInfo == MARK {
                SWALLOWED.fetch_add(1, Ordering::SeqCst);
                return LRESULT(1);
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_INPUT {
            let mut raw = RAWINPUT::default();
            let mut size = std::mem::size_of::<RAWINPUT>() as u32;
            // SAFETY: a buffer of RAWINPUT's size for a mouse report.
            let got = unsafe {
                GetRawInputData(
                    HRAWINPUT(lp.0 as _),
                    RID_INPUT,
                    Some(&mut raw as *mut _ as *mut _),
                    &mut size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                )
            };
            if got != u32::MAX
                && raw.header.dwType == RIM_TYPEMOUSE.0
                && unsafe { raw.data.mouse.ulExtraInformation } as usize == MARK
            {
                RAW_SEEN.fetch_add(1, Ordering::SeqCst);
            }
        }
        unsafe { DefWindowProcW(h, msg, wp, lp) }
    }

    fn pump(ms: u64) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut msg = MSG::default();
        while std::time::Instant::now() < until {
            // SAFETY: standard message pump on this thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    pub fn run(n: usize, with_hook: bool) -> Counts {
        SWALLOWED.store(0, Ordering::SeqCst);
        RAW_SEEN.store(0, Ordering::SeqCst);
        // SAFETY: plain Win32 setup on this thread, torn down below.
        unsafe {
            let hinst = GetModuleHandleW(None).expect("module handle");
            let class = w!("OxdeskRawInputProbe");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("probe"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinst.into()),
                None,
            )
            .expect("message-only window");
            let dev = RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            };
            RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
                .expect("register raw input");
            let hk = if with_hook {
                Some(SetWindowsHookExW(WH_MOUSE_LL, Some(hook), Some(hinst.into()), 0).expect("hook"))
            } else {
                None
            };
            pump(100);

            let mut injected = 0;
            for i in 0..n {
                let dx = if i % 2 == 0 { 1 } else { -1 };
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT { dx, dy: 0, dwFlags: MOUSEEVENTF_MOVE, dwExtraInfo: MARK, ..Default::default() },
                    },
                };
                injected += SendInput(&[input], std::mem::size_of::<INPUT>() as i32) as usize;
                pump(5);
            }
            pump(300);

            if let Some(h) = hk {
                let _ = UnhookWindowsHookEx(h);
            }
            let _ = DestroyWindow(hwnd);
            Counts {
                injected,
                swallowed: SWALLOWED.load(Ordering::SeqCst),
                raw_seen: RAW_SEEN.load(Ordering::SeqCst),
            }
        }
    }
}
