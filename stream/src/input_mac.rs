//! Keyboard and mouse injection on macOS, through CoreGraphics.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Needs Accessibility, and there is no way around it
//!
//! `CGEventPost` only reaches other programs if the process has the
//! **Accessibility** permission. Without it the events are accepted and
//! silently discarded -- the call returns success and nothing happens.
//! It is the same failure mode as Screen Recording: macOS does not warn,
//! it just ignores.
//!
//! That is why the host **checks at startup** and says what is missing,
//! instead of letting the user find out by moving the mouse and seeing
//! nothing.
//!
//! ## Raw FFI, no crate
//!
//! Three functions and two opaque types. A bindings crate would bring
//! hundreds of symbols we do not use and one more version to keep up
//! with; `winput` on the Windows side followed the same reasoning.

#![cfg(target_os = "macos")]

use crate::input::{mac_key_from_evdev, Event, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};

type CFTypeRef = *const std::ffi::c_void;
type CGEventRef = *mut std::ffi::c_void;
type CGEventSourceRef = *mut std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

// Mouse event types, from `CGEventType`.
const MOUSE_MOVED: u32 = 5;
const L_DOWN: u32 = 1;
const L_UP: u32 = 2;
const R_DOWN: u32 = 3;
const R_UP: u32 = 4;
const OTHER_DOWN: u32 = 25;
const OTHER_UP: u32 = 26;
const L_DRAG: u32 = 6;
const R_DRAG: u32 = 7;

// Buttons, from `CGMouseButton`.
const BTN_L: u32 = 0;
const BTN_R: u32 = 1;
const BTN_M: u32 = 2;

// `CGEventField` fields for the relative delta.
const DELTA_X: u32 = 4;
const DELTA_Y: u32 = 5;

type CGDisplayModeRef = *mut std::ffi::c_void;

/// `kCGHIDEventTap`: enters the queue as if it came from the hardware.
const TAP_HID: u32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        kind: u32,
        pos: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(
        source: CGEventSourceRef,
        key: u16,
        pressed: bool,
    ) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent(
        source: CGEventSourceRef,
        unit: u32,
        axes: u32,
        a: i32,
        b: i32,
    ) -> CGEventRef;
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGMainDisplayID() -> u32;
    // The display mode is the only place that reports BOTH sizes of the
    // same screen. `CGDisplayPixelsWide`, despite its name, returns
    // POINTS (measured: 1680 on a display whose mode has 3360 pixels).
    fn CGDisplayCopyDisplayMode(display: u32) -> CGDisplayModeRef;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeRelease(mode: CGDisplayModeRef);
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(obj: CFTypeRef);
}

/// Can the process inject input?
///
/// Public because the host is the one that warns, at startup: a KVM that
/// accepts events and silently discards them is worse than one that refuses.
pub fn has_accessibility() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Capture pixels per point, read from the display's video mode.
///
/// `CGDisplayPixelsWide` does NOT work for this: despite its name it
/// returns points. Measured on the MacBook Air -- 1680 on a display whose
/// mode declares 3360 pixels. That naming confusion is what let the bug
/// through: the edge clamp uses the right function by accident, and only
/// the absolute path broke.
///
/// Returns 1.0 when it cannot tell. See `point_from_pixel`.
pub fn display_scale() -> f64 {
    unsafe {
        let mode = CGDisplayCopyDisplayMode(CGMainDisplayID());
        if mode.is_null() {
            return 1.0;
        }
        let px = CGDisplayModeGetPixelWidth(mode) as f64;
        let pt = CGDisplayModeGetWidth(mode) as f64;
        CGDisplayModeRelease(mode);
        if px > 0.0 && pt > 0.0 {
            px / pt
        } else {
            1.0
        }
    }
}

pub struct Injector {
    source: CGEventSourceRef,
    /// Last known pointer position.
    ///
    /// Kept by us because RELATIVE movement needs a starting point, and
    /// asking the system on every event would cost one more call on the hot
    /// path. Synced with the real one at creation and whenever an absolute
    /// position arrives.
    pos: CGPoint,
    /// Pressed buttons, so a drag comes out as a drag.
    left: bool,
    right: bool,
    /// Capture pixels per CoreGraphics point. 2.0 on a Retina.
    ///
    /// Read once: changing the video mode in the middle of a session would
    /// bring the capture down anyway, and the host starts again.
    scale: f64,
}

impl Injector {
    pub fn new() -> Self {
        // 1 = kCGEventSourceStateHIDSystemState: the modifier state comes
        // from the real keyboard, so a Shift the user is holding locally
        // is not lost.
        let source = unsafe { CGEventSourceCreate(1) };
        let pos = unsafe {
            let e = CGEventCreate(std::ptr::null_mut());
            if e.is_null() {
                CGPoint { x: 0.0, y: 0.0 }
            } else {
                let p = CGEventGetLocation(e);
                CFRelease(e as CFTypeRef);
                p
            }
        };
        Injector { source, pos, left: false, right: false, scale: display_scale() }
    }

    pub fn apply(&mut self, e: Event) {
        match e {
            Event::MouseRel { dx, dy } => self.move_relative(dx, dy),
            Event::Mouse { x, y } => self.move_absolute(x, y),
            Event::Button { code, pressed } => self.button(code, pressed),
            Event::Scroll { dx, dy } => self.scroll(dx, dy),
            Event::Key { keycode, pressed } => self.key(keycode, pressed),
        }
    }

    /// Posts an event and releases it. Forgetting the release leaks per event.
    fn post(&self, ev: CGEventRef) {
        if ev.is_null() {
            return;
        }
        unsafe {
            CGEventPost(TAP_HID, ev);
            CFRelease(ev as CFTypeRef);
        }
    }

    fn move_relative(&mut self, dx: f64, dy: f64) {
        let (w, h) = unsafe {
            let d = CGMainDisplayID();
            (CGDisplayPixelsWide(d) as f64, CGDisplayPixelsHigh(d) as f64)
        };
        // Clamp to the screen: without this the accumulated position leaves
        // the display and the pointer "sticks" to an edge until the user
        // moves the same amount back -- it looks frozen, and it is not.
        self.pos.x = (self.pos.x + dx).clamp(0.0, w - 1.0);
        self.pos.y = (self.pos.y + dy).clamp(0.0, h - 1.0);

        // Dragging? The event type changes, otherwise the system reads
        // "move with button pressed" as a plain move and the drag does not
        // happen.
        let kind = if self.left {
            L_DRAG
        } else if self.right {
            R_DRAG
        } else {
            MOUSE_MOVED
        };
        let ev = unsafe { CGEventCreateMouseEvent(self.source, kind, self.pos, BTN_L) };
        if !ev.is_null() {
            // The delta has to go EXPLICITLY: a game that reads raw input
            // looks at this field, not at the position difference. Without
            // it, the aim does not move even with the pointer moving.
            unsafe {
                CGEventSetIntegerValueField(ev, DELTA_X, dx as i64);
                CGEventSetIntegerValueField(ev, DELTA_Y, dy as i64);
            }
        }
        self.post(ev);
    }

    fn move_absolute(&mut self, x: f64, y: f64) {
        // The frame comes in pixels; CGEvent wants points. See
        // `point_from_pixel` and the why in stream/src/input.rs.
        let (x, y) = crate::input::point_from_pixel(x, y, self.scale);
        self.pos = CGPoint { x, y };
        let ev = unsafe { CGEventCreateMouseEvent(self.source, MOUSE_MOVED, self.pos, BTN_L) };
        self.post(ev);
    }

    fn button(&mut self, code: i32, pressed: bool) {
        let (kind, button) = match (code, pressed) {
            (c, true) if c == BTN_LEFT => {
                self.left = true;
                (L_DOWN, BTN_L)
            }
            (c, false) if c == BTN_LEFT => {
                self.left = false;
                (L_UP, BTN_L)
            }
            (c, true) if c == BTN_RIGHT => {
                self.right = true;
                (R_DOWN, BTN_R)
            }
            (c, false) if c == BTN_RIGHT => {
                self.right = false;
                (R_UP, BTN_R)
            }
            (c, true) if c == BTN_MIDDLE => (OTHER_DOWN, BTN_M),
            (c, false) if c == BTN_MIDDLE => (OTHER_UP, BTN_M),
            _ => return,
        };
        let ev = unsafe { CGEventCreateMouseEvent(self.source, kind, self.pos, button) };
        self.post(ev);
    }

    fn scroll(&self, dx: f64, dy: f64) {
        // Unit 0 = pixel. The protocol sends scroll "pixels" and macOS
        // accepts the same unit, so there is no conversion.
        //
        // Inverted sign: in our protocol (and in Wayland) positive Y goes
        // down; on macOS, positive goes up.
        let ev = unsafe {
            CGEventCreateScrollWheelEvent(self.source, 0, 2, -dy as i32, dx as i32)
        };
        self.post(ev);
    }

    fn key(&self, keycode: u32, pressed: bool) {
        // No translation, nothing is sent: posting an arbitrary keycode
        // would type a WRONG letter on someone else's machine, which is
        // worse than not typing.
        let Some(mac) = mac_key_from_evdev(keycode) else { return };
        let ev = unsafe { CGEventCreateKeyboardEvent(self.source, mac, pressed) };
        self.post(ev);
    }
}

// The event source lives as long as the host lives; there is no path in
// which an `Injector` is dropped and recreated, so there is no Drop. If one
// day there is, it needs `CFRelease(self.source)`.

/// Thread that owns the `Injector`, fed by a channel.
///
/// CoreGraphics types are raw pointers and therefore `!Send`, and the
/// host's network task is `tokio::spawn` -- which requires `Send`. We could
/// declare `unsafe impl Send` betting that `CGEventPost` is thread-safe,
/// but that is a claim that cannot be verified from here.
///
/// An owning thread solves it with no bet at all, and it is the same shape
/// the Linux host already uses for the D-Bus connection (which is also
/// `!Send` and whose session belongs to the connection that created it).
/// The cost is one channel hop per event -- microseconds, against ~300 ms
/// of network.
pub fn thread() -> std::sync::mpsc::Sender<Event> {
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    std::thread::spawn(move || {
        let mut t = Injector::new();
        while let Ok(e) = rx.recv() {
            t.apply(e);
        }
    });
    tx
}

// ------------------------------------------------- permissions, no guessing
//
// TCC ties the grant to the BINARY (to its designated requirement), not to
// the user nor to the app that asks. So only this executable can answer
// whether IT has permission: `oxdesk` asking on its own account would give a
// right answer about the wrong program.
//
// That is what makes `--conferir` necessary instead of a function in the
// daemon.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Says whether there is Screen Recording permission. Does **not** show a dialog.
    fn CGPreflightScreenCaptureAccess() -> bool;
    /// Asks for the permission. Shows the system dialog ONCE in the
    /// binary's lifetime; after that it returns what was already decided,
    /// silently.
    fn CGRequestScreenCaptureAccess() -> bool;
}

pub fn has_screen_recording() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Asks for the two permissions the host needs.
///
/// Runs in the HOST process on purpose, and has to be launched from the
/// graphical session: over SSH there is no dialog to show, and the call
/// returns `false` without anyone having seen anything
/// ([[../../.claude/rules/graphical-session]]).
///
/// Accessibility uses the variant with `kAXTrustedCheckOptionPrompt`, which
/// is what makes macOS open the right pane instead of just answering "no".
/// Without the prompt the person would have to find the item in the list
/// by hand, and the name shown there is `oxdesk-host`, not "oxdesk".
pub fn request_permissions() -> (bool, bool) {
    let screen = unsafe { CGRequestScreenCaptureAccess() };
    let input = unsafe {
        // Byte string with an explicit NUL, and not the `c"..."` literal:
        // the literal needs rustc 1.77+, and this file only compiles ON THE
        // MAC, where the toolchain version is different and I had no way to
        // check before sending. A hand-written `\0` compiles on any version
        // and costs a comment.
        let key = CFStringCreateWithCString(
            std::ptr::null(),
            b"AXTrustedCheckOptionPrompt\0".as_ptr() as *const std::ffi::c_char,
            0x0800_0100, // kCFStringEncodingUTF8
        );
        let options = CFDictionaryCreate(
            std::ptr::null(),
            [key].as_ptr(),
            [kCFBooleanTrue].as_ptr(),
            1,
            std::ptr::null(),
            std::ptr::null(),
        );
        let r = AXIsProcessTrustedWithOptions(options);
        CFRelease(options as CFTypeRef);
        CFRelease(key as CFTypeRef);
        r
    };
    (screen, input)
}

type CFStringRef = *const std::ffi::c_void;
type CFDictionaryRef = *const std::ffi::c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    // The name is Apple's symbol; renaming it would break the link.
    #[allow(non_upper_case_globals)]
    static kCFBooleanTrue: *const std::ffi::c_void;
    fn CFStringCreateWithCString(
        alloc: *const std::ffi::c_void,
        cstr: *const std::ffi::c_char,
        enc: u32,
    ) -> CFStringRef;
    fn CFDictionaryCreate(
        alloc: *const std::ffi::c_void,
        keys: *const CFStringRef,
        values: *const *const std::ffi::c_void,
        n: isize,
        kcb: *const std::ffi::c_void,
        vcb: *const std::ffi::c_void,
    ) -> CFDictionaryRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
}
