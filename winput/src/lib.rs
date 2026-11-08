#![cfg(windows)]
//! Win32 code shared by `oxdesk` and `oxdesk-agent`.
//!
//! It lived inside the agent as `input.rs`. It became a crate when the
//! daemon started running on Windows too: two copies of `unsafe` FFI that
//! must agree on session and single instance are exactly the kind of
//! duplication that silences a bug on one of the sides.
//!
//! Age of the last input in the interactive Windows session.
//!
//! It is the dead-man's switch signal (spec 005): after receiving the
//! device, if no input arrives within N seconds, the agent returns the
//! channel on its own -- because the SOURCE machine cannot undo it.
//!
//! GetLastInputInfo is per SESSION: a service in session 0 does not see
//! the user's input. That is why the guard has to run in the interactive
//! session (shortcut in shell:startup), not as a service.
//!
//! Error strings stay pt-BR: they can reach the panel.

#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[link(name = "user32")]
extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetTickCount64() -> u64;
}

/// Milliseconds since the last input (keyboard or mouse) in this session.
pub fn idle_ms() -> Result<u64, String> {
    let mut lii = LastInputInfo {
        cb_size: std::mem::size_of::<LastInputInfo>() as u32,
        dw_time: 0,
    };
    let ok = unsafe { GetLastInputInfo(&mut lii) };
    if ok == 0 {
        return Err("GetLastInputInfo falhou".into());
    }
    let now = unsafe { GetTickCount64() };
    // dw_time is 32-bit and wraps every ~49 days; GetTickCount64 does not.
    // Truncating now to 32 bits and subtracting with wrapping gives the
    // correct difference even across the wrap.
    Ok((now as u32).wrapping_sub(lii.dw_time) as u64)
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[link(name = "user32")]
extern "system" {
    fn GetCursorPos(p: *mut Point) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

// Virtual screen: covers multi-monitor, unlike SM_CXSCREEN which only
// reports the primary monitor.
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

pub fn cursor() -> Option<Point> {
    let mut p = Point::default();
    if unsafe { GetCursorPos(&mut p) } == 0 {
        return None;
    }
    Some(p)
}

/// (x, y, width, height) of the virtual screen.
pub fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Is the cursor touching edge `edge`?
///
/// On Windows we need neither a portal nor a compositor barrier: reading
/// GetCursorPos is enough, which is why this side is simpler than the
/// GNOME one. 1 px tolerance because the cursor stops at the last valid
/// column.
pub fn at_edge(edge: &str, p: Point) -> bool {
    let (x, y, w, h) = virtual_screen();
    match edge {
        "left" => p.x <= x,
        "right" => p.x >= x + w - 1,
        "top" => p.y <= y,
        "bottom" => p.y >= y + h - 1,
        _ => false,
    }
}


#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: *const u8, initial_owner: i32, name: *const u16) -> isize;
    fn GetLastError() -> u32;
    fn WTSGetActiveConsoleSessionId() -> u32;
    fn ProcessIdToSessionId(pid: u32, sid: *mut u32) -> i32;
    fn GetCurrentProcessId() -> u32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;

/// Are we in the physical console session?
///
/// The resident modes ONLY work there: GetLastInputInfo and GetCursorPos
/// are per session. Running in session 0 (the case of launching over SSH)
/// is not harmless -- that instance sees no input at all, reports a fake
/// 1024x768 desktop and an idle equal to the uptime, and still competes
/// for the HID++ receiver with the real instance.
pub fn in_console_session() -> bool {
    let console = unsafe { WTSGetActiveConsoleSessionId() };
    let mut mine: u32 = u32::MAX;
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut mine) };
    if ok == 0 || console == u32::MAX {
        // Could not tell: carrying on is better than refusing.
        return true;
    }
    mine == console
}

/// Ensures a single instance of the resident modes.
///
/// Without this, the Startup shortcut plus a manual launch stack up: it
/// already happened -- three `watch-edge` at once, competing for the same
/// receiver and tripling the HID++ traffic.
///
/// The handle is deliberately leaked: the mutex must live as long as the
/// process does, and Windows releases it on exit.
pub fn claim_single_instance(name: &str) -> bool {
    // Local\ and not Global\: Global requires SeCreateGlobalPrivilege, and
    // when creation fails there is no telling "already exists" from "could
    // not create" -- measured, two instances got through. The
    // cross-session case is already blocked by `in_console_session`, so
    // the per-session namespace is exactly the scope left to cover.
    let wide: Vec<u16> = format!("Local\\{name}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, wide.as_ptr()) };
    if h == 0 {
        // Could not create the mutex: carrying on is better than not running.
        return true;
    }
    unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
}

// --- clipboard -------------------------------------------------------
//
// Exists so `oxdesk` does not have to call `powershell -Command
// Get-Clipboard` on every sample. That worked, but spawning a PowerShell
// every 2 s costs ~200 ms of CPU per read -- and, if the caller forgets
// `CREATE_NO_WINDOW`, it opens a visible console window on every round.
// That is what happened: "o windows ta abrindo um powershell a cada 5s"
// (Windows is opening a PowerShell every 5 s).
//
// Unicode text only (CF_UNICODETEXT). Files are CF_HDROP and are handled
// by the file clipboard section below.

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
extern "system" {
    fn OpenClipboard(h: isize) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(format: u32) -> isize;
    fn SetClipboardData(format: u32, mem: isize) -> isize;
    fn IsClipboardFormatAvailable(format: u32) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
    fn GlobalLock(mem: isize) -> *mut u16;
    fn GlobalUnlock(mem: isize) -> i32;
    fn GlobalFree(mem: isize) -> isize;
}

/// Opens the clipboard, runs `f`, and closes it even if `f` fails.
///
/// Closing is mandatory: the clipboard is a GLOBAL resource, and leaving
/// it open blocks Ctrl+C and Ctrl+V for every program in the session
/// until the process dies. That is why no exit path skips
/// `CloseClipboard`.
fn with_clipboard<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    // The clipboard may be held by another program for a few ms. Five
    // attempts 20 ms apart cover the normal case without hanging the
    // caller when someone really holds it.
    let mut opened = false;
    for _ in 0..5 {
        if unsafe { OpenClipboard(0) } != 0 {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return Err("o clipboard esta ocupado por outro programa".into());
    }
    let r = f();
    unsafe { CloseClipboard() };
    r
}

/// Clipboard text, or `None` if there is no text in it.
pub fn clipboard_read() -> Result<Option<String>, String> {
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 {
        return Ok(None);
    }
    with_clipboard(|| {
        let mem = unsafe { GetClipboardData(CF_UNICODETEXT) };
        if mem == 0 {
            return Ok(None);
        }
        let p = unsafe { GlobalLock(mem) };
        if p.is_null() {
            return Err("GlobalLock falhou na leitura".into());
        }
        // Zero-terminated UTF-16. Walking to the terminator is the only
        // way: the block size is rounded up and may carry trailing garbage.
        let mut n = 0usize;
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
        unsafe { GlobalUnlock(mem) };
        Ok(Some(s))
    })
}

/// Puts text on the clipboard.
pub fn clipboard_write(text: &str) -> Result<(), String> {
    let mut u: Vec<u16> = text.encode_utf16().collect();
    u.push(0);
    let bytes = u.len() * 2;

    with_clipboard(|| {
        let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if mem == 0 {
            return Err("GlobalAlloc falhou".into());
        }
        let p = unsafe { GlobalLock(mem) };
        if p.is_null() {
            unsafe { GlobalFree(mem) };
            return Err("GlobalLock falhou na escrita".into());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(u.as_ptr(), p, u.len());
            GlobalUnlock(mem);
        }
        unsafe { EmptyClipboard() };
        // After SetClipboardData the block belongs to the SYSTEM -- freeing
        // it here would be use-after-free on the next Ctrl+V. We only free
        // it when the Set FAILS and ownership did not change hands.
        if unsafe { SetClipboardData(CF_UNICODETEXT, mem) } == 0 {
            unsafe { GlobalFree(mem) };
            return Err("SetClipboardData falhou".into());
        }
        Ok(())
    })
}

// --- file clipboard --------------------------------------------------
//
// Files on the Windows clipboard are `CF_HDROP`: a `DROPFILES` followed
// by a list of UTF-16 paths, each zero-terminated and the whole list
// terminated by an extra zero. It is not text, and the `CF_UNICODETEXT`
// path cannot be reused.

const CF_HDROP: u32 = 15;

#[repr(C)]
struct DropFiles {
    /// Offset of the start of the name list, from this struct.
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    /// Are the coordinates in the non-client area? Unused; stays 0.
    f_nc: i32,
    /// List in UTF-16? Always 1 here -- the ANSI version would lose accents.
    f_wide: i32,
}

#[link(name = "shell32")]
extern "system" {
    fn DragQueryFileW(drop: isize, index: u32, buf: *mut u16, size: u32) -> u32;
}

/// File paths on the clipboard, or `None` if there are no files.
pub fn clipboard_read_files() -> Result<Option<Vec<String>>, String> {
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) } == 0 {
        return Ok(None);
    }
    with_clipboard(|| {
        let h = unsafe { GetClipboardData(CF_HDROP) };
        if h == 0 {
            return Ok(None);
        }
        // 0xFFFF_FFFF asks for the COUNT instead of a name.
        let n = unsafe { DragQueryFileW(h, 0xFFFF_FFFF, std::ptr::null_mut(), 0) };
        let mut v = Vec::with_capacity(n as usize);
        for i in 0..n {
            // Size first, then the name: asking with a short buffer
            // truncates silently, and a truncated path points to another
            // file -- or to none.
            let size = unsafe { DragQueryFileW(h, i, std::ptr::null_mut(), 0) };
            if size == 0 {
                continue;
            }
            let mut buf = vec![0u16; size as usize + 1];
            let read = unsafe { DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32) };
            if read == 0 {
                continue;
            }
            v.push(String::from_utf16_lossy(&buf[..read as usize]));
        }
        Ok(if v.is_empty() { None } else { Some(v) })
    })
}

/// Puts a list of files on the clipboard.
pub fn clipboard_write_files(paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("lista de arquivos vazia".into());
    }
    // DROPFILES + UTF-16 names, each with \0, plus a final \0.
    let mut names: Vec<u16> = Vec::new();
    for c in paths {
        names.extend(c.encode_utf16());
        names.push(0);
    }
    names.push(0);

    let header = std::mem::size_of::<DropFiles>();
    let bytes = header + names.len() * 2;

    with_clipboard(|| {
        let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) };
        if mem == 0 {
            return Err("GlobalAlloc falhou".into());
        }
        let p = unsafe { GlobalLock(mem) } as *mut u8;
        if p.is_null() {
            unsafe { GlobalFree(mem) };
            return Err("GlobalLock falhou".into());
        }
        unsafe {
            let df = p as *mut DropFiles;
            std::ptr::write(
                df,
                DropFiles { p_files: header as u32, pt_x: 0, pt_y: 0, f_nc: 0, f_wide: 1 },
            );
            std::ptr::copy_nonoverlapping(names.as_ptr(), p.add(header) as *mut u16, names.len());
            GlobalUnlock(mem);
            EmptyClipboard();
        }
        // Same rule as for text: after the Set the block is the SYSTEM's.
        if unsafe { SetClipboardData(CF_HDROP, mem) } == 0 {
            unsafe { GlobalFree(mem) };
            return Err("SetClipboardData(CF_HDROP) falhou".into());
        }
        Ok(())
    })
}
