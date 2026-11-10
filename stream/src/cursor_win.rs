#![cfg(windows)]
//! Drawing the cursor on the Windows DESKTOP, not into the frame.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why this exists on top of `cursor.rs`
//!
//! `cursor.rs` composes an arrow INTO the captured frame, which serves
//! whoever is watching this screen in the viewer. It does nothing for the
//! case that actually hurt: the receiver left this machine, the person
//! drives it from the other side through the LAN KVM and looks at THIS
//! monitor, with their own eyes. There is no frame of ours in that path --
//! and Windows draws no cursor at all, because with no pointing device
//! installed it hides the pointer. Measured with `GetCursorInfo`:
//! `flags = 0`, `hCursor = 0`, and still the correct position. Hover
//! effects answered and nothing showed up.
//!
//! So we draw it: a layered, click-through window that follows
//! `GetCursorPos`. Being a real desktop window it appears on the physical
//! monitor AND is captured by DXGI like any other window -- which is why
//! `capture_win` stops composing while this is up. Two arrows for the
//! viewer would be worse than none.
//!
//! ## It only draws while someone is driving
//!
//! A fullscreen game hides the pointer ON PURPOSE, and an arrow we paint
//! over it is us intruding. So the lifetime is the input session, not the
//! host's: `driving(true)` when an input connection arrives, `false` when
//! it leaves. Nobody driving, nothing drawn.
//!
//! The counter is not a bool because there are TWO doors for input (the
//! viewer sends over the video connection, the KVM over the input port).
//! With a bool, whichever disconnected first would turn off the arrow of
//! whoever was still typing.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, GetDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorInfo, GetCursorPos, PeekMessageW,
    RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, CURSORINFO,
    CURSOR_SHOWING, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// How many input connections are open. Above zero, someone is driving.
static DRIVING: AtomicI32 = AtomicI32::new(0);

/// The overlay is on screen RIGHT NOW.
///
/// `capture_win` reads this to stop composing its own arrow: ours is
/// already in the desktop image DXGI hands over.
static DRAWING: AtomicBool = AtomicBool::new(false);

/// The thread exists. Starting twice would register the class twice and
/// leave an orphan window following the cursor forever.
static UP: AtomicBool = AtomicBool::new(false);

/// An input connection opened (`true`) or closed (`false`).
pub fn driving(on: bool) {
    if on {
        DRIVING.fetch_add(1, Ordering::Relaxed);
    } else {
        DRIVING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Is our arrow on screen? Read by `capture_win`, which must not draw a
/// second one.
pub fn drawing() -> bool {
    DRAWING.load(Ordering::Relaxed)
}

/// Starts the overlay thread. Idempotent.
///
/// A thread of its own because a window belongs to the thread that created
/// it and needs its own message pump -- and the host's main thread is busy
/// capturing at 60 Hz.
pub fn start() {
    if UP.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        if let Err(e) = overlay() {
            // Loud, and then out of the way: a machine with no arrow is
            // usable, a host that died because of the arrow is not.
            eprintln!("  cursor overlay: off ({e})");
            DRAWING.store(false, Ordering::Relaxed);
        }
    });
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// Poll interval while someone is driving.
///
/// 8 ms is 125 Hz: above the 60 Hz of the screen, so the arrow never lags
/// behind the frame that shows it. `GetCursorPos` is cheap and we only
/// touch the window when the position actually changed.
const MOVING_MS: u64 = 8;

/// And while nobody is driving. There is nothing to follow; this is just
/// how long it takes to notice that someone arrived.
const IDLE_MS: u64 = 60;

fn overlay() -> anyhow::Result<()> {
    // Env var and not a config field: this is tuned BY EYE, on the machine,
    // and a rebuild-and-redeploy cycle to find out that 1 was too small
    // would be absurd. 1 is the classic arrow's size.
    let scale = std::env::var("OXDESK_CURSOR_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1);
    let (aw, ah, px) = crate::cursor::arrow_bgra(scale);

    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("LogiCursorOverlay");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(anyhow::anyhow!("RegisterClassW refused the class"));
        }

        // The four `WS_EX` that make it a cursor and not a window:
        // LAYERED for per-pixel alpha, TRANSPARENT so clicks go through to
        // what is underneath, NOACTIVATE so it never steals focus (it would
        // steal it from whatever the person is typing into), TOOLWINDOW so
        // it stays out of Alt-Tab and the taskbar.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!("oxdesk cursor"),
            WS_POPUP,
            0,
            0,
            aw as i32,
            ah as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;

        let screen = GetDC(None);
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = aw as i32;
        // NEGATIVE height = top-down rows, which is the order `arrow_bgra`
        // returns. With the positive default the arrow comes out upside
        // down -- and an upside-down arrow still looks like a cursor, which
        // is the kind of bug you stare at for an hour.
        bi.bmiHeader.biHeight = -(ah as i32);
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(screen), &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        if bits.is_null() {
            return Err(anyhow::anyhow!("CreateDIBSection gave no pixels"));
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mem = CreateCompatibleDC(Some(screen));
        SelectObject(mem, bmp.into());

        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 0xFF,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE { cx: aw as i32, cy: ah as i32 };
        let src = POINT { x: 0, y: 0 };

        println!("  cursor overlay: ready, {aw}x{ah} (scale {scale})");

        let mut shown = false;
        let mut last = (i32::MIN, i32::MIN);
        // Far enough in the past that the first visible iteration raises.
        let mut raised = std::time::Instant::now() - crate::cursor::RAISE_EVERY;
        let mut msg = MSG::default();
        loop {
            // The pump is mandatory even for a window nobody clicks: an
            // unresponsive window makes the desktop wait on us.
            while PeekMessageW(&mut msg, Some(hwnd), 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            let mut ci = CURSORINFO {
                cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            // When the system HAS a cursor to draw, it draws better than we
            // do (text bar, hand, resize arrows). We only fill the hole.
            let system_drawing =
                GetCursorInfo(&mut ci).is_ok() && (ci.flags.0 & CURSOR_SHOWING.0) != 0;
            let mut pt = POINT::default();
            let have_pos = GetCursorPos(&mut pt).is_ok();
            let want = DRIVING.load(Ordering::Relaxed) > 0 && !system_drawing && have_pos;

            if want {
                if (pt.x, pt.y) != last {
                    last = (pt.x, pt.y);
                    let dst = POINT { x: pt.x, y: pt.y };
                    // `UpdateLayeredWindow` moves AND paints in one call,
                    // which is what keeps the arrow from tearing: position
                    // and pixels never belong to different instants.
                    let _ = UpdateLayeredWindow(
                        hwnd,
                        Some(screen),
                        Some(&dst),
                        Some(&size),
                        Some(mem),
                        Some(&src),
                        COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    );
                }
                let appearing = !shown;
                if appearing {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    shown = true;
                    DRAWING.store(true, Ordering::Relaxed);
                    println!("  cursor overlay: drawing at {},{}", pt.x, pt.y);
                }
                // Topmost again, and AGAIN AND AGAIN. Doing it only on
                // appearance covered the window that went fullscreen while
                // we were hidden, and missed the one that rises while we
                // are visible -- which is the taskbar, every time the
                // pointer reaches it. See `cursor::should_raise`.
                if crate::cursor::should_raise(appearing, raised.elapsed()) {
                    let _ = SetWindowPos(
                        hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    raised = std::time::Instant::now();
                }
            } else if shown {
                let _ = ShowWindow(hwnd, SW_HIDE);
                shown = false;
                DRAWING.store(false, Ordering::Relaxed);
                println!(
                    "  cursor overlay: hidden (system drawing: {system_drawing}, \
                     driving: {})",
                    DRIVING.load(Ordering::Relaxed)
                );
            }

            std::thread::sleep(std::time::Duration::from_millis(if want {
                MOVING_MS
            } else {
                IDLE_MS
            }));
        }
    }
}
