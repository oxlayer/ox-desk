// SPDX-FileCopyrightText: 2026 oxlayer
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Input events the client sends back to the host.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why a stream and not datagrams
//!
//! Video tolerates loss; input does not. Dropping a "key up" leaves the key
//! stuck on the host, and dropping a "button up" leaves the mouse dragging
//! on its own. That is why input goes over a QUIC **stream** -- reliable and
//! ordered -- while video goes over datagrams.
//!
//! ## Fixed format per type
//!
//! One type byte and a known size per type. No length field: the reader
//! knows how many bytes to ask for as soon as it reads the type, and there
//! is no way for a wrong number to make it read the next event from the middle.

use bytes::{Buf, BufMut, BytesMut};

/// evdev button codes, which is what Mutter expects.
pub const BTN_LEFT: i32 = 0x110;
pub const BTN_RIGHT: i32 = 0x111;
pub const BTN_MIDDLE: i32 = 0x112;
/// The two side buttons ("back" and "forward"). Windows calls them
/// `XBUTTON1` and `XBUTTON2`; Mutter takes the evdev codes.
pub const BTN_SIDE: i32 = 0x113;
pub const BTN_EXTRA: i32 = 0x114;

/// The way back from a grab: two keys held together, as evdev codes.
///
/// Both Ctrl keys unless the person picked another pair in the panel
/// (`[input.hotkey] keys`, spec 023). The daemon passes the pair to
/// `oxdesk-kvm` as `--rescue <a>,<b>`, and it is the same pair that sends
/// the input away, so the keys that took it are the keys that bring it home.
pub const RESCUE_DEFAULT: (u32, u32) = (29, 97); // KEY_LEFTCTRL, KEY_RIGHTCTRL

/// Reads `--rescue`'s value, `"<a>,<b>"`. `None` for anything else: a
/// typo must not become a rescue nobody can press.
pub fn parse_chord(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once(',')?;
    let (a, b) = (a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?);
    // Two different keys, both inside the keyboard range: a chord of one
    // key would fire on every press of it.
    (a != b && (1..=255).contains(&a) && (1..=255).contains(&b)).then_some((a, b))
}

/// Are both keys of the chord held?
pub fn chord_held(pressed: &std::collections::HashSet<u32>, chord: (u32, u32)) -> bool {
    pressed.contains(&chord.0) && pressed.contains(&chord.1)
}

// --- what a host holds on a client's behalf (spec 025 section 6) -----------

/// The keys and buttons a host pressed for a client and has not released.
///
/// A source that exits cleanly sends a key-up for whatever is held (the KVM
/// does, in `finish`). One that is killed, crashes or loses the network in
/// the middle of a press sends nothing more -- and the key stays down on the
/// target: Shift held forever, typing in capitals until someone notices
/// (spec 005 records it happening on the viewer path). So the host keeps
/// its own account and, when the input stream ends for any reason, releases
/// what is left.
#[derive(Default)]
pub struct Held {
    keys: std::collections::BTreeSet<u32>,
    buttons: std::collections::BTreeSet<i32>,
}

impl Held {
    /// Records an event the host is about to apply.
    pub fn note(&mut self, e: &Event) {
        match *e {
            Event::Key { keycode, pressed } => {
                if pressed {
                    self.keys.insert(keycode);
                } else {
                    self.keys.remove(&keycode);
                }
            }
            Event::Button { code, pressed } => {
                if pressed {
                    self.buttons.insert(code);
                } else {
                    self.buttons.remove(&code);
                }
            }
            _ => {}
        }
    }

    /// The events that release everything still held -- buttons first, so a
    /// drag ends where it is -- and forgets it all.
    pub fn release_all(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> =
            self.buttons.iter().map(|&code| Event::Button { code, pressed: false }).collect();
        out.extend(self.keys.iter().map(|&keycode| Event::Key { keycode, pressed: false }));
        self.keys.clear();
        self.buttons.clear();
        out
    }
}

// --- the way back by the screen edge (spec 024) ---------------------------

/// The line a host sends a KVM client when its input connection opens, on
/// a stream of its own: `oxdesk-size <width> <height>`. The size of the
/// screen the input lands on, in the pixels absolute positions use.
///
/// Text and a stream of its own, not an `Event`: it goes the other way, it
/// is sent once, and a client from before it never accepts the stream and
/// loses nothing.
pub fn size_line(w: u32, h: u32) -> String {
    format!("oxdesk-size {w} {h}\n")
}

pub fn parse_size_line(s: &str) -> Option<(u32, u32)> {
    let mut f = s.trim().split_whitespace();
    if f.next()? != "oxdesk-size" {
        return None;
    }
    let (w, h) = (f.next()?.parse::<u32>().ok()?, f.next()?.parse::<u32>().ok()?);
    (w > 0 && h > 0 && f.next().is_none()).then_some((w, h))
}

/// A screen edge, as the layout names them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub fn parse(s: &str) -> Option<Edge> {
        Some(match s {
            "left" => Edge::Left,
            "right" => Edge::Right,
            "top" => Edge::Top,
            "bottom" => Edge::Bottom,
            _ => return None,
        })
    }
}

/// How far past the edge the mouse has to be pushed to come back, in
/// pixels of the other screen. Not the first pixel: the scroll bar of a
/// maximised window lives on the right edge, and reaching for it must not
/// send the input home.
pub const RETURN_PUSH: f64 = 40.0;
/// How far inside the other screen the cursor appears.
pub const ENTRY_INSET: f64 = 20.0;

/// Where the cursor comes back in, by this machine's edge `edge`, at
/// `along` (0..1) of the desktop's height (left/right) or width (top/bottom),
/// `inset` pixels inside a real monitor.
///
/// Mapping onto the bounding box of all monitors put the cursor off every
/// screen when the monitors differ in size: on the reference Windows
/// machine, coming back low on the left landed below the 1080-high left
/// monitor (spec 025 section 3). So: the outermost monitor that covers that
/// height, or the nearest one when none does.
pub fn return_point(edge: Edge, along: f64, monitors: &[(i32, i32, i32, i32)], inset: i32) -> Option<(i32, i32)> {
    if monitors.is_empty() {
        return None;
    }
    let along = along.clamp(0.0, 1.0);
    let span = |lo: i32, hi: i32| lo + (along * f64::from((hi - lo - 1).max(0))).round() as i32;
    let top = monitors.iter().map(|m| m.1).min()?;
    let bottom = monitors.iter().map(|m| m.1 + m.3).max()?;
    let left = monitors.iter().map(|m| m.0).min()?;
    let right = monitors.iter().map(|m| m.0 + m.2).max()?;
    // Distance from a coordinate to a monitor's span on one axis.
    let gap = |v: i32, lo: i32, len: i32| if v < lo { lo - v } else if v >= lo + len { v - (lo + len - 1) } else { 0 };
    let inside = |v: i32, lo: i32, len: i32| v.clamp(lo, lo + len - 1);
    let inward = |lo: i32, len: i32, from_low: bool| {
        let i = inset.min((len - 1) / 2).max(0);
        if from_low { lo + i } else { lo + len - 1 - i }
    };
    Some(match edge {
        Edge::Left | Edge::Right => {
            let y = span(top, bottom);
            let m = monitors.iter().min_by_key(|m| {
                let outer = if edge == Edge::Left { m.0 } else { -(m.0 + m.2) };
                (gap(y, m.1, m.3), outer)
            })?;
            (inward(m.0, m.2, edge == Edge::Left), inside(y, m.1, m.3))
        }
        Edge::Top | Edge::Bottom => {
            let x = span(left, right);
            let m = monitors.iter().min_by_key(|m| {
                let outer = if edge == Edge::Top { m.1 } else { -(m.1 + m.3) };
                (gap(x, m.0, m.2), outer)
            })?;
            (inside(x, m.0, m.2), inward(m.1, m.3, edge == Edge::Top))
        }
    })
}

/// What to do with one event from the source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Send(Event),
    /// The mouse was pushed past the return edge: bring the input home.
    /// The value is where along that edge, 0..1, so the cursor can come
    /// back at the same height.
    Return(f64),
}

/// The cursor on the OTHER machine, kept by the source.
///
/// Relative motion leaves the source blind: the other side clamps at its
/// screen edges, and nothing comes back to say where the cursor is. So,
/// once the other side has said its size, the source keeps the position
/// itself and sends absolute positions -- and then it knows when the
/// cursor is at the edge that faces home, and how hard it is being pushed.
///
/// Until the size arrives (or with no return edge, or a host that never
/// sends one) it passes relative motion through unchanged: the behaviour
/// from before.
pub struct Pointer {
    edge: Option<Edge>,
    /// Where the cursor enters along the edge, 0..1 (the height it left
    /// this machine at).
    entry: f64,
    size: Option<(f64, f64)>,
    x: f64,
    y: f64,
    push: f64,
}

impl Pointer {
    pub fn new(edge: Option<Edge>, entry: f64) -> Pointer {
        Pointer { edge, entry: entry.clamp(0.0, 1.0), size: None, x: 0.0, y: 0.0, push: 0.0 }
    }

    pub fn tracking(&self) -> bool {
        self.edge.is_some() && self.size.is_some()
    }

    /// Is a size still useful to this pointer? Without a return edge it
    /// never is, and the caller can stop looking for one.
    pub fn wants_size(&self) -> bool {
        self.edge.is_some() && self.size.is_none()
    }

    /// The other screen's size arrived. Places the cursor just inside the
    /// return edge -- the side this machine is on -- and returns the
    /// absolute position to send so the other cursor jumps there now.
    pub fn set_size(&mut self, w: u32, h: u32) -> Option<Event> {
        let edge = self.edge?;
        let (w, h) = (w as f64, h as f64);
        self.size = Some((w, h));
        let along = |len: f64| (self.entry * (len - 1.0)).round();
        (self.x, self.y) = match edge {
            Edge::Right => (w - 1.0 - ENTRY_INSET, along(h)),
            Edge::Left => (ENTRY_INSET, along(h)),
            Edge::Bottom => (along(w), h - 1.0 - ENTRY_INSET),
            Edge::Top => (along(w), ENTRY_INSET),
        };
        Some(Event::Mouse { x: self.x, y: self.y })
    }

    pub fn map(&mut self, e: Event) -> Step {
        let (Some(edge), Some((w, h)), Event::MouseRel { dx, dy }) = (self.edge, self.size, e) else {
            return Step::Send(e);
        };
        let (nx, ny) = (self.x + dx, self.y + dy);
        // How far past the return edge this movement tried to go.
        let over = match edge {
            Edge::Right => nx - (w - 1.0),
            Edge::Left => -nx,
            Edge::Bottom => ny - (h - 1.0),
            Edge::Top => -ny,
        };
        self.x = nx.clamp(0.0, w - 1.0);
        self.y = ny.clamp(0.0, h - 1.0);
        if over > 0.0 {
            self.push += over;
            if self.push >= RETURN_PUSH {
                self.push = 0.0;
                let along = match edge {
                    Edge::Left | Edge::Right => self.y / (h - 1.0).max(1.0),
                    Edge::Top | Edge::Bottom => self.x / (w - 1.0).max(1.0),
                };
                return Step::Return(along);
            }
        } else {
            // Moving back in, or along the edge without pushing: start over.
            self.push = 0.0;
        }
        Step::Send(Event::Mouse { x: self.x, y: self.y })
    }
}

/// Scroll pixels in one line of text -- the unit `Event::Scroll` carries.
///
/// It lives here, with the protocol, because it is a **wire** unit: every
/// end has to agree on it, and until 08/11 two of them did not. The viewer
/// sends `LineDelta * 15` and the Windows injector divides by 15; the LAN
/// KVM sent 120 px per notch -- Windows' own wheel unit -- so one click of
/// the wheel scrolled EIGHT lines there. Measured on the Windows machine.
///
/// 15 px is not a measurement of anything, it is a choice: Mutter wants a
/// continuous offset and a line of text is about that tall. What matters is
/// that there is exactly one of it.
pub const SCROLL_PX_PER_LINE: f64 = 15.0;

/// Whole wheel notches in `px` of scroll, with the fraction left in `rem`
/// for the next call.
///
/// Windows counts the wheel in notches, not in pixels, so this is where the
/// unit changes. It lives in this module and not next to `SendInput` so that
/// a test can reach it: `input_win.rs` is `#![cfg(windows)]`, and a test
/// written in there never runs in `scripts/check.sh` -- which is exactly how
/// the eightfold error above survived.
///
/// The remainder is not a detail: a trackpad sends a fraction of a line per
/// event, and truncating each one on its own would never reach a notch, so
/// the page would not move at all.
pub fn wheel_notches(px: f64, rem: &mut f64) -> i32 {
    *rem += px / SCROLL_PX_PER_LINE;
    let whole = rem.trunc();
    *rem -= whole;
    whole as i32
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Absolute position in the space of the captured stream (pixels).
    ///
    /// Absolute rather than relative because the client knows exactly where
    /// its cursor is inside the window, and converting to relative would
    /// accumulate error on every frame.
    Mouse { x: f64, y: f64 },
    /// Relative pointer movement, in pixels.
    ///
    /// The LAN KVM sends this, not `Mouse`. Two reasons:
    ///
    /// - **Games.** Mouse-look reads relative movement; with absolute
    ///   position the aim gets stuck at the screen edges. It is the use case
    ///   that motivated the KVM ("switch to Windows to play").
    /// - **No need to know the size of the other side.** The viewer knows
    ///   where the cursor is inside the window and sends absolute; the KVM
    ///   has no window at all, and asking for the remote resolution just to
    ///   convert would be one more handshake to make the result worse.
    MouseRel { dx: f64, dy: f64 },
    Button { code: i32, pressed: bool },
    /// Scroll offset in pixels, `SCROLL_PX_PER_LINE` to a line of text.
    ///
    /// Positive `dy` scrolls DOWN, as in Wayland. Every injector whose
    /// target counts the wheel upwards inverts it on the way in.
    Scroll { dx: f64, dy: f64 },
    /// evdev keycode (what `NotifyKeyboardKeycode` receives).
    Key { keycode: u32, pressed: bool },
}

impl Event {
    const T_MOUSE: u8 = 1;
    const T_BUTTON: u8 = 2;
    const T_SCROLL: u8 = 3;
    const T_KEY: u8 = 4;
    const T_MOUSE_REL: u8 = 5;

    /// Payload bytes of each type, not counting the type byte.
    fn size(tag: u8) -> Option<usize> {
        Some(match tag {
            Self::T_MOUSE => 16,
            Self::T_BUTTON => 5,
            Self::T_SCROLL => 16,
            Self::T_KEY => 5,
            Self::T_MOUSE_REL => 16,
            _ => return None,
        })
    }

    pub fn write(&self, b: &mut BytesMut) {
        match *self {
            Event::Mouse { x, y } => {
                b.put_u8(Self::T_MOUSE);
                b.put_f64(x);
                b.put_f64(y);
            }
            Event::Button { code, pressed } => {
                b.put_u8(Self::T_BUTTON);
                b.put_i32(code);
                b.put_u8(u8::from(pressed));
            }
            Event::Scroll { dx, dy } => {
                b.put_u8(Self::T_SCROLL);
                b.put_f64(dx);
                b.put_f64(dy);
            }
            Event::Key { keycode, pressed } => {
                b.put_u8(Self::T_KEY);
                b.put_u32(keycode);
                b.put_u8(u8::from(pressed));
            }
            Event::MouseRel { dx, dy } => {
                b.put_u8(Self::T_MOUSE_REL);
                b.put_f64(dx);
                b.put_f64(dy);
            }
        }
    }
}

/// Accumulates stream bytes and hands out whole events.
///
/// A stream does not respect message boundaries: one `read` may bring half
/// an event or three and a half. Without this accumulator, half an event
/// would become a wrong command -- and a wrong command here moves someone's
/// computer.
#[derive(Default)]
pub struct Reader {
    buf: BytesMut,
    /// Bytes discarded because of an unknown type. If this grows, the two
    /// ends are on different protocol versions.
    pub unknown: u64,
}

impl Reader {
    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    pub fn next_event(&mut self) -> Option<Event> {
        loop {
            if self.buf.is_empty() {
                return None;
            }
            let tag = self.buf[0];
            let Some(n) = Event::size(tag) else {
                // A type we do not know: skipping 1 byte and trying again
                // is better than stalling the stream forever.
                self.buf.advance(1);
                self.unknown += 1;
                continue;
            };
            if self.buf.len() < 1 + n {
                return None; // incomplete event: wait for more bytes
            }
            self.buf.advance(1);
            let e = match tag {
                Event::T_MOUSE => {
                    Event::Mouse { x: self.buf.get_f64(), y: self.buf.get_f64() }
                }
                Event::T_BUTTON => Event::Button {
                    code: self.buf.get_i32(),
                    pressed: self.buf.get_u8() != 0,
                },
                Event::T_SCROLL => {
                    Event::Scroll { dx: self.buf.get_f64(), dy: self.buf.get_f64() }
                }
                Event::T_MOUSE_REL => {
                    Event::MouseRel { dx: self.buf.get_f64(), dy: self.buf.get_f64() }
                }
                Event::T_KEY => Event::Key {
                    keycode: self.buf.get_u32(),
                    pressed: self.buf.get_u8() != 0,
                },
                // Unreachable: `size()` already refused any tag outside the
                // list, and we only get here with the bytes complete.
                // Even so it does NOT fall into `Key`, which is what used to
                // be written -- a `_ =>` that invents a type reads the wrong
                // number of bytes and desynchronizes the whole stream from
                // there on. With a 16-byte tag read as a 5-byte key, 11
                // bytes are left over and become phantom commands on
                // someone else's computer. Discarding the whole payload is
                // the only safe way out.
                other => {
                    self.buf.advance(n);
                    self.unknown += 1;
                    debug_assert!(false, "tag {other} passed size() without an arm");
                    continue;
                }
            };
            return Some(e);
        }
    }
}

#[cfg(test)]
mod codec_tests {
    use super::*;

    /// Round trip of every type, including the new relative one.
    #[test]
    fn every_event_round_trips() {
        let samples = [
            Event::Mouse { x: 12.5, y: -3.25 },
            Event::MouseRel { dx: -4.0, dy: 7.5 },
            Event::Button { code: BTN_LEFT, pressed: true },
            Event::Scroll { dx: 0.0, dy: -120.0 },
            Event::Key { keycode: 30, pressed: false },
        ];
        let mut b = BytesMut::new();
        for e in &samples {
            e.write(&mut b);
        }
        let mut l = Reader::default();
        l.feed(&b);
        let read: Vec<_> = std::iter::from_fn(|| l.next_event()).collect();
        assert_eq!(read, samples, "the codec did not round-trip");
        assert_eq!(l.unknown, 0);
    }

    /// A new type must NOT be read as another one.
    ///
    /// The decoder used to end in `_ => Key`, so any tag it did not know
    /// read 5 bytes as a key. When `MouseRel` (16 bytes) came in, that arm
    /// would have read 5 and left 11 bytes dangling -- and from then on the
    /// WHOLE stream becomes phantom commands on someone else's computer.
    /// This test locks the format.
    #[test]
    fn sixteen_byte_tag_does_not_misalign_what_follows() {
        let mut b = BytesMut::new();
        Event::MouseRel { dx: 1.0, dy: 2.0 }.write(&mut b);
        Event::Key { keycode: 57, pressed: true }.write(&mut b);
        let mut l = Reader::default();
        l.feed(&b);
        assert_eq!(l.next_event(), Some(Event::MouseRel { dx: 1.0, dy: 2.0 }));
        assert_eq!(l.next_event(), Some(Event::Key { keycode: 57, pressed: true }));
        assert_eq!(l.next_event(), None);
    }

    /// A truly unknown tag disappears without taking the rest with it.
    #[test]
    fn unknown_tag_is_skipped() {
        let mut b = BytesMut::new();
        b.put_u8(200); // does not exist
        Event::Key { keycode: 1, pressed: true }.write(&mut b);
        let mut l = Reader::default();
        l.feed(&b);
        assert_eq!(l.next_event(), Some(Event::Key { keycode: 1, pressed: true }));
        assert_eq!(l.unknown, 1);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_of_scroll_is_one_wheel_notch() {
        let mut rem = 0.0;
        assert_eq!(wheel_notches(SCROLL_PX_PER_LINE, &mut rem), 1);
        assert_eq!(wheel_notches(-SCROLL_PX_PER_LINE, &mut rem), -1);
    }

    #[test]
    fn a_notch_from_the_kvm_is_one_notch_on_windows() {
        // The regression. The KVM used to put 120 here -- Windows' wheel
        // unit, not the protocol's pixel -- and 120/15 is eight. The page
        // jumped eight lines per click of the wheel.
        let mut rem = 0.0;
        assert_eq!(wheel_notches(120.0, &mut rem), 8);
        assert_eq!(wheel_notches(15.0, &mut rem), 1);
    }

    #[test]
    fn fractions_of_a_line_accumulate_instead_of_vanishing() {
        // The trackpad case: five events of a fifth of a line each have to
        // add up to one notch, not to nothing.
        let mut rem = 0.0;
        let fifth = SCROLL_PX_PER_LINE / 5.0;
        for _ in 0..4 {
            assert_eq!(wheel_notches(fifth, &mut rem), 0);
        }
        assert_eq!(wheel_notches(fifth, &mut rem), 1);
    }

    fn all() -> Vec<Event> {
        vec![
            Event::Mouse { x: 1280.5, y: 720.25 },
            Event::Button { code: BTN_LEFT, pressed: true },
            Event::Button { code: BTN_LEFT, pressed: false },
            Event::Scroll { dx: 0.0, dy: -3.5 },
            Event::Key { keycode: 30, pressed: true },
        ]
    }

    #[test]
    fn round_trip() {
        let mut b = BytesMut::new();
        for e in all() {
            e.write(&mut b);
        }
        let mut l = Reader::default();
        l.feed(&b);
        let read: Vec<_> = std::iter::from_fn(|| l.next_event()).collect();
        assert_eq!(read, all());
        assert_eq!(l.unknown, 0);
    }

    #[test]
    fn stream_split_in_the_middle_of_an_event() {
        // The case that matters: a TCP/QUIC stream does not respect message
        // boundaries. Half an event interpreted would be a wrong command on
        // someone's machine.
        let mut b = BytesMut::new();
        for e in all() {
            e.write(&mut b);
        }
        let bytes = b.to_vec();
        let mut l = Reader::default();
        let mut read = Vec::new();
        // One byte at a time: the worst possible slicing.
        for byte in &bytes {
            l.feed(&[*byte]);
            while let Some(e) = l.next_event() {
                read.push(e);
            }
        }
        assert_eq!(read, all());
    }

    #[test]
    fn unknown_type_does_not_stall_the_stream() {
        // Different versions on the two ends: what is not understood is
        // skipped, and what comes after still counts.
        let mut b = BytesMut::new();
        b.put_u8(99); // a type from the future
        Event::Key { keycode: 57, pressed: true }.write(&mut b);
        let mut l = Reader::default();
        l.feed(&b);
        assert_eq!(l.next_event(), Some(Event::Key { keycode: 57, pressed: true }));
        assert_eq!(l.unknown, 1);
    }
}

/// Translator from physical key to evdev keycode.
///
/// `winit` delivers `KeyCode`, which follows the W3C names (`KeyA`,
/// `Digit1`); Mutter wants the evdev number. There is no formula: it is a
/// table.
///
/// **Physical, not logical**, on purpose. Sending the character would let
/// the client's layout rule the host -- pressing `A` on an ABNT2 keyboard
/// would become something else on a machine with a US layout. By sending
/// the key's POSITION, each machine applies its own layout, which is how a
/// real keyboard behaves.
#[cfg(feature = "winit-keys")]
pub fn evdev_from_winit(code: winit::keyboard::KeyCode) -> Option<u32> {
    use winit::keyboard::KeyCode as K;
    Some(match code {
        K::Escape => 1,
        K::Digit1 => 2, K::Digit2 => 3, K::Digit3 => 4, K::Digit4 => 5, K::Digit5 => 6,
        K::Digit6 => 7, K::Digit7 => 8, K::Digit8 => 9, K::Digit9 => 10, K::Digit0 => 11,
        K::Minus => 12, K::Equal => 13, K::Backspace => 14, K::Tab => 15,
        K::KeyQ => 16, K::KeyW => 17, K::KeyE => 18, K::KeyR => 19, K::KeyT => 20,
        K::KeyY => 21, K::KeyU => 22, K::KeyI => 23, K::KeyO => 24, K::KeyP => 25,
        K::BracketLeft => 26, K::BracketRight => 27, K::Enter => 28, K::ControlLeft => 29,
        K::KeyA => 30, K::KeyS => 31, K::KeyD => 32, K::KeyF => 33, K::KeyG => 34,
        K::KeyH => 35, K::KeyJ => 36, K::KeyK => 37, K::KeyL => 38,
        K::Semicolon => 39, K::Quote => 40, K::Backquote => 41, K::ShiftLeft => 42,
        K::Backslash => 43,
        K::KeyZ => 44, K::KeyX => 45, K::KeyC => 46, K::KeyV => 47, K::KeyB => 48,
        K::KeyN => 49, K::KeyM => 50,
        K::Comma => 51, K::Period => 52, K::Slash => 53, K::ShiftRight => 54,
        K::NumpadMultiply => 55, K::AltLeft => 56, K::Space => 57, K::CapsLock => 58,
        K::F1 => 59, K::F2 => 60, K::F3 => 61, K::F4 => 62, K::F5 => 63,
        K::F6 => 64, K::F7 => 65, K::F8 => 66, K::F9 => 67, K::F10 => 68,
        K::NumLock => 69, K::ScrollLock => 70,
        K::Numpad7 => 71, K::Numpad8 => 72, K::Numpad9 => 73, K::NumpadSubtract => 74,
        K::Numpad4 => 75, K::Numpad5 => 76, K::Numpad6 => 77, K::NumpadAdd => 78,
        K::Numpad1 => 79, K::Numpad2 => 80, K::Numpad3 => 81, K::Numpad0 => 82,
        K::NumpadDecimal => 83,
        K::IntlBackslash => 86, K::F11 => 87, K::F12 => 88,
        // The key to the right of `M` on an ABNT2 keyboard.
        K::IntlRo => 89,
        K::NumpadEnter => 96, K::ControlRight => 97, K::NumpadDivide => 98,
        K::PrintScreen => 99, K::AltRight => 100,
        K::Home => 102, K::ArrowUp => 103, K::PageUp => 104, K::ArrowLeft => 105,
        K::ArrowRight => 106, K::End => 107, K::ArrowDown => 108, K::PageDown => 109,
        K::Insert => 110, K::Delete => 111,
        K::AudioVolumeMute => 113, K::AudioVolumeDown => 114, K::AudioVolumeUp => 115,
        K::Pause => 119,
        K::SuperLeft => 125, K::SuperRight => 126, K::ContextMenu => 127,
        _ => return None,
    })
}

/// Translates an evdev keycode to an AT Set 1 scancode.
///
/// Returns `(scancode, needs the E0 prefix)`. Today the user is the Windows
/// host, which injects via `KEYEVENTF_SCANCODE` -- but the table lives HERE,
/// next to its inverse (`evdev_from_winit`), and not in the platform module:
/// it is a pure table, and in the `cfg(windows)` module its tests would never
/// run on the machine where it is written. A table without tests that run is
/// a table that gets a key wrong silently.
///
/// Almost identity by a happy accident: the first 83 evdev codes were
/// COPIED from Set 1 (evdev 30 = `A` = 0x1E = 30). What the table handles is
/// what departed from that -- the `E0`-prefix keys (arrows, Home, right Ctrl,
/// the two Windows keys) and the ABNT2 layout ones.
pub fn scancode_from_evdev(keycode: u32) -> Option<(u16, bool)> {
    Some(match keycode {
        // Identity: evdev codes up to 88 are the Set 1 ones.
        // 84 and 85 do not exist on a common keyboard and the client never sends them.
        1..=83 | 86..=88 => (keycode as u16, false),
        // KEY_RO: the key to the right of `M` on ABNT2 and Japanese keyboards.
        89 => (0x73, false),
        96 => (0x1C, true),  // numeric keypad Enter
        97 => (0x1D, true),  // right Ctrl
        98 => (0x35, true),  // keypad /
        99 => (0x37, true),  // PrintScreen
        100 => (0x38, true), // right Alt (AltGr)
        102 => (0x47, true), // Home
        103 => (0x48, true), // arrow up
        104 => (0x49, true), // PageUp
        105 => (0x4B, true), // arrow left
        106 => (0x4D, true), // arrow right
        107 => (0x4F, true), // End
        108 => (0x50, true), // arrow down
        109 => (0x51, true), // PageDown
        110 => (0x52, true), // Insert
        111 => (0x53, true), // Delete
        113 => (0x20, true), // mute
        114 => (0x2E, true), // volume -
        115 => (0x30, true), // volume +
        125 => (0x5B, true), // left Windows
        126 => (0x5C, true), // right Windows
        127 => (0x5D, true), // context menu
        // 119 (Pause) goes out as a virtual key: its scancode is the
        // prefix sequence `E1 1D 45`, which SendInput cannot express.
        _ => return None,
    })
}

/// The inverse of `scancode_from_evdev`: what a Windows keyboard hook
/// reports, as the evdev code the protocol carries.
///
/// Derived from the table above rather than written a second time, so the
/// two directions cannot drift apart: a key the Windows host knows how to
/// type is, by construction, a key the Windows KVM source knows how to send.
///
/// The two keys Set 1 cannot tell apart by scancode (Pause and NumLock
/// both end in `45`) are told apart by the caller, by virtual key.
pub fn evdev_from_scancode(scan: u16, extended: bool) -> Option<u32> {
    (1..=127u32).find(|k| scancode_from_evdev(*k) == Some((scan, extended)))
}

/// What a Windows low-level keyboard hook reports (`vkCode`, `scanCode`,
/// the extended flag), as an evdev code.
///
/// Pure, and here rather than in `grab_win.rs`, so its tests run on every
/// machine (see `scancode_from_evdev`).
///
/// - Pause and NumLock share scancode `45`; the virtual key tells them apart.
/// - `scanCode` is taken whole, never cut to a byte: the Ctrl that Windows
///   fakes in front of AltGr arrives as `0x21D`, and cut to `0x1D` it would
///   become a real left Ctrl on the other machine -- and half of the rescue
///   chord.
pub fn evdev_from_windows_key(vk: u32, scan: u32, extended: bool) -> Option<u32> {
    match vk {
        0x13 => return Some(119), // VK_PAUSE -> KEY_PAUSE
        0x90 => return Some(69),  // VK_NUMLOCK -> KEY_NUMLOCK
        _ => {}
    }
    let scan = u16::try_from(scan).ok()?;
    if scan > 0xFF {
        return None;
    }
    evdev_from_scancode(scan, extended)
}

/// Moves a point at least `inset` pixels inside a rectangle
/// `(left, top, width, height)`.
///
/// For the cursor coming back from a grab: put back exactly where it left,
/// it would sit on the screen edge that sent the input away, and the edge
/// watcher would read that as a new arrival and send it away again.
pub fn inset_from_edges(p: (i32, i32), rect: (i32, i32, i32, i32), inset: i32) -> (i32, i32) {
    let (left, top, w, h) = rect;
    if w <= 2 * inset || h <= 2 * inset {
        return p;
    }
    (p.0.clamp(left + inset, left + w - 1 - inset), p.1.clamp(top + inset, top + h - 1 - inset))
}

#[cfg(all(test, feature = "winit-keys"))]
mod key_tests {
    use super::*;
    use winit::keyboard::KeyCode as K;

    #[test]
    fn common_keys_have_a_code() {
        // A sample covering letter, digit, modifier, arrow and function key.
        for (k, expected) in [
            (K::KeyA, 30),
            (K::Digit1, 2),
            (K::Space, 57),
            (K::Enter, 28),
            (K::ShiftLeft, 42),
            (K::ControlLeft, 29),
            (K::SuperLeft, 125),
            (K::ArrowUp, 103),
            (K::F5, 63),
            (K::Escape, 1),
        ] {
            assert_eq!(evdev_from_winit(k), Some(expected), "{k:?}");
        }
    }

    #[test]
    fn unmapped_key_returns_nothing() {
        // Better to ignore than to send a wrong code: a wrong code presses
        // ANOTHER key on someone's machine.
        assert_eq!(evdev_from_winit(K::Fn), None);
    }
}

#[cfg(test)]
mod scancode_tests {
    use super::*;

    #[test]
    fn letters_and_digits_are_identity() {
        // The historical accident the table depends on: evdev 1..83 == Set 1.
        assert_eq!(scancode_from_evdev(30), Some((0x1E, false))); // A
        assert_eq!(scancode_from_evdev(2), Some((0x02, false))); // 1
        assert_eq!(scancode_from_evdev(57), Some((0x39, false))); // space
        assert_eq!(scancode_from_evdev(28), Some((0x1C, false))); // Enter
    }

    #[test]
    fn extended_keys_ask_for_the_prefix() {
        // Without E0, `arrow up` would become keypad `8` -- same scancode,
        // different meaning.
        assert_eq!(scancode_from_evdev(103), Some((0x48, true)));
        assert_eq!(scancode_from_evdev(72), Some((0x48, false)));
        assert_eq!(scancode_from_evdev(97), Some((0x1D, true))); // right Ctrl
        assert_eq!(scancode_from_evdev(29), Some((0x1D, false))); // left Ctrl
    }


    /// evdev codes the table does NOT translate, and why.
    ///
    /// An explicit list on purpose: it is what makes the coverage test fail
    /// when someone teaches the client to send a new key and forgets the
    /// host side. Without the list, the gap would look "normal".
    const UNTRANSLATED: &[(u32, &str)] = &[
        (84, "does not exist on a common keyboard"),
        (85, "zenkaku/hankaku, Japanese keyboard"),
        (90, "katakana"),
        (91, "hiragana"),
        (92, "henkan"),
        (93, "katakana/hiragana"),
        (94, "muhenkan"),
        (95, "Japanese keypad comma"),
        (101, "linefeed, does not exist on current physical keyboards"),
        (112, "macro"),
        (116, "power"),
        (117, "keypad ="),
        (118, "keypad +/-"),
        (119, "Pause: its scancode is the sequence E1 1D 45; goes as a virtual key"),
        (120, "scale"),
        (121, "keypad comma"),
        (122, "hangeul"),
        (123, "hanja"),
        (124, "yen"),
    ];

    #[test]
    fn every_key_the_client_sends_has_a_translation() {
        // The invariant that matters: if the viewer can send a code, the
        // Windows host must know what to do with it. A gap here raises no
        // error -- the key simply does nothing, and that gets discovered on
        // someone's keyboard.
        for c in 1u32..=127 {
            let exception = UNTRANSLATED.iter().find(|(k, _)| *k == c);
            match (scancode_from_evdev(c), exception) {
                (Some(_), None) => {}
                (None, Some(_)) => {}
                (Some(_), Some((_, reason))) => {
                    panic!("evdev {c} is translated but listed as untranslated ({reason})")
                }
                (None, None) => panic!(
                    "evdev {c} has no scancode and is not in the exception list -- \
                     if the client sends that key, it does nothing on Windows"
                ),
            }
        }
    }

    #[test]
    fn two_keys_never_become_the_same() {
        // A collision would be worse than a gap: the wrong key presses
        // ANOTHER key on someone's machine, and the symptom does not point here.
        let mut seen: std::collections::BTreeMap<(u16, bool), u32> = Default::default();
        for c in 1u32..=127 {
            if let Some(pair) = scancode_from_evdev(c) {
                if let Some(before) = seen.insert(pair, c) {
                    panic!("evdev {before} and {c} produce the same scancode {pair:?}");
                }
            }
        }
    }

    #[test]
    fn pause_has_no_scancode() {
        // Handled as a virtual key; returning Some here would inject another one.
        assert_eq!(scancode_from_evdev(119), None);
    }
}

// --- capture pixel -> injection point ---------------------------------

/// Converts an ABSOLUTE frame position to the space in which the target
/// system injects input.
///
/// The protocol carries `Event::Mouse` in **frame pixels**: the viewer
/// knows the size of what it receives and maps the window onto it
/// (`remote_point`, in bin/view.rs). But not every system injects in that
/// same space.
///
/// On macOS the difference is a factor of 2 and it is visible: `CGEvent`
/// works in **points**, and on a Retina a 1680x1050-point screen is
/// captured at 3360x2100 pixels. Sending the pixel directly takes the
/// pointer to twice the position -- pointing at the middle of the window
/// throws the cursor to the bottom-right corner. It was reported as "the
/// cursor position on the mac did not match".
///
/// It lives here, platform-neutral, for the same reason as
/// `mac_key_from_evdev`: in `input_mac.rs` the test would never run on the
/// machine that runs the suite.
///
/// `scale` = capture pixels per point. `1.0` on a normal screen, `2.0`
/// on a Retina. Zero or negative converts nothing: it returns the point
/// as it came, because guessing a factor would be worse than not scaling.
pub fn point_from_pixel(x: f64, y: f64, scale: f64) -> (f64, f64) {
    if scale <= 0.0 {
        return (x, y);
    }
    (x / scale, y / scale)
}

// --- keys on macOS ---------------------------------------------------

/// evdev keycode -> macOS virtual keycode (`kVK_*`).
///
/// The project's third table, for the same reason as the other two: the
/// protocol carries **evdev** on every path (it is what Mutter's
/// `NotifyKeyboardKeycode` receives), and each system has its own
/// numbering. Windows needs Set-1 (`scancode_from_evdev`); macOS needs
/// `kVK`, which has no relation at all to either of the other two -- `A` is
/// 30 in evdev, 0x1E in Set-1 and 0x00 on macOS.
///
/// It lives here, platform-neutral, and not in `input_mac.rs`, because
/// that file is `cfg(target_os = "macos")` and the tests of this table
/// would never run on the machine that runs the suite. Same decision that
/// already took `scancode_from_evdev` out of `input_win.rs` and the cursor
/// compositing out of `capture_win.rs`.
pub fn mac_key_from_evdev(evdev: u32) -> Option<u16> {
    Some(match evdev {
        // Letters. The macOS order is neither alphabetical nor keyboard
        // order -- it is the Apple II hardware order onwards, so the table
        // is literal out of necessity.
        30 => 0x00, // A
        48 => 0x0B, // B
        46 => 0x08, // C
        32 => 0x02, // D
        18 => 0x0E, // E
        33 => 0x03, // F
        34 => 0x05, // G
        35 => 0x04, // H
        23 => 0x22, // I
        36 => 0x26, // J
        37 => 0x28, // K
        38 => 0x25, // L
        50 => 0x2E, // M
        49 => 0x2D, // N
        24 => 0x1F, // O
        25 => 0x23, // P
        16 => 0x0C, // Q
        19 => 0x0F, // R
        31 => 0x01, // S
        20 => 0x11, // T
        22 => 0x20, // U
        47 => 0x09, // V
        17 => 0x0D, // W
        45 => 0x07, // X
        21 => 0x10, // Y
        44 => 0x06, // Z

        // Top-row digits.
        2 => 0x12,  // 1
        3 => 0x13,  // 2
        4 => 0x14,  // 3
        5 => 0x15,  // 4
        6 => 0x17,  // 5
        7 => 0x16,  // 6
        8 => 0x1A,  // 7
        9 => 0x1C,  // 8
        10 => 0x19, // 9
        11 => 0x1D, // 0

        // Punctuation and basic controls.
        12 => 0x1B, // -
        13 => 0x18, // =
        14 => 0x33, // backspace (kVK_Delete on macOS)
        15 => 0x30, // tab
        26 => 0x21, // [
        27 => 0x1E, // ]
        28 => 0x24, // enter
        39 => 0x29, // ;
        40 => 0x27, // '
        41 => 0x32, // `
        43 => 0x2A, // backslash
        51 => 0x2B, // ,
        52 => 0x2F, // .
        53 => 0x2C, // /
        57 => 0x31, // space
        1 => 0x35,  // esc
        58 => 0x39, // caps lock
        111 => 0x75, // delete (forward)

        // Modifiers. macOS separates left and right.
        29 => 0x3B,  // left ctrl
        97 => 0x3E,  // right ctrl
        42 => 0x38,  // left shift
        54 => 0x3C,  // right shift
        56 => 0x3A,  // left alt/option
        100 => 0x3D, // right alt/option
        125 => 0x37, // left meta -> command
        126 => 0x36, // right meta -> right command

        // Arrows and navigation.
        103 => 0x7E, // up
        108 => 0x7D, // down
        105 => 0x7B, // left
        106 => 0x7C, // right
        102 => 0x73, // home
        107 => 0x77, // end
        104 => 0x74, // page up
        109 => 0x79, // page down

        // Function keys. The macOS numbering jumps on purpose -- F1 is 0x7A
        // and F5 is 0x60; it cannot be computed, only listed.
        59 => 0x7A, // F1
        60 => 0x78, // F2
        61 => 0x63, // F3
        62 => 0x76, // F4
        63 => 0x60, // F5
        64 => 0x61, // F6
        65 => 0x62, // F7
        66 => 0x64, // F8
        67 => 0x65, // F9
        68 => 0x6D, // F10
        87 => 0x67, // F11
        88 => 0x6F, // F12

        _ => return None,
    })
}

#[cfg(test)]
mod mac_key_tests {
    use super::*;

    /// A sample covering letter, digit, modifier, arrow and function key.
    #[test]
    fn common_keys_have_a_translation() {
        let cases = [
            (30u32, 0x00u16, "A"),
            (17, 0x0D, "W"),
            (2, 0x12, "1"),
            (28, 0x24, "enter"),
            (57, 0x31, "space"),
            (14, 0x33, "backspace"),
            (42, 0x38, "left shift"),
            (125, 0x37, "command"),
            (103, 0x7E, "arrow up"),
            (59, 0x7A, "F1"),
            (63, 0x60, "F5"),
        ];
        for (evdev, mac, name) in cases {
            assert_eq!(mac_key_from_evdev(evdev), Some(mac), "{name}");
        }
    }

    /// Two evdev keys cannot land on the same kVK.
    ///
    /// A collision would make two different keys type the same thing on
    /// the other machine -- and, being a hand-written table, it is the most
    /// likely mistake here.
    #[test]
    fn no_collision_in_the_table() {
        let mut seen: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
        for evdev in 0u32..=130 {
            let Some(mac) = mac_key_from_evdev(evdev) else { continue };
            if let Some(before) = seen.insert(mac, evdev) {
                panic!("evdev {before} and {evdev} land on the same kVK {mac:#04x}");
            }
        }
    }

    /// A code that does not exist does not become a key.
    #[test]
    fn unknown_returns_none() {
        assert_eq!(mac_key_from_evdev(9999), None);
    }

    /// The KVM rescue chord must be translatable on both sides.
    #[test]
    fn both_ctrls_exist() {
        assert!(mac_key_from_evdev(29).is_some(), "left ctrl");
        assert!(mac_key_from_evdev(97).is_some(), "right ctrl");
    }
}

#[cfg(test)]
mod point_from_pixel_tests {
    use super::*;

    #[test]
    fn retina_divides_by_two() {
        // The real case: a MacBook Air captured at 3360x2100, injecting
        // into a 1680x1050-point space. The corner must become the corner.
        assert_eq!(point_from_pixel(3359.0, 2099.0, 2.0), (1679.5, 1049.5));
        // And the middle must become the middle -- this is where the cursor
        // used to go to the bottom-right corner.
        assert_eq!(point_from_pixel(1680.0, 1050.0, 2.0), (840.0, 525.0));
    }

    #[test]
    fn normal_screen_is_untouched() {
        assert_eq!(point_from_pixel(1920.0, 1080.0, 1.0), (1920.0, 1080.0));
    }

    #[test]
    fn invalid_scale_returns_as_is() {
        // If the display query fails, scaling by 0 would send the pointer
        // to infinity (or to NaN). Passing straight through is worse on a
        // Retina, but it is predictable and does not break a normal screen.
        assert_eq!(point_from_pixel(100.0, 200.0, 0.0), (100.0, 200.0));
        assert_eq!(point_from_pixel(100.0, 200.0, -2.0), (100.0, 200.0));
    }
}

#[cfg(test)]
mod reverse_key_tests {
    use super::*;

    /// The reference Windows machine: 1920x1080 left of the 2560x1440 primary.
    const REF: [(i32, i32, i32, i32); 2] = [(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)];

    #[test]
    fn coming_back_lands_on_a_real_monitor() {
        // High up: the left monitor is the outermost at that height.
        assert_eq!(return_point(Edge::Left, 0.3, &REF, 60), Some((-1860, 432)));
        // Low down: only the primary covers it -- not the box's left edge,
        // which is off every screen there.
        assert_eq!(return_point(Edge::Left, 0.9, &REF, 60), Some((60, 1295)));
        // The right side is the primary's at any height.
        assert_eq!(return_point(Edge::Right, 0.3, &REF, 60), Some((2499, 432)));
        assert_eq!(return_point(Edge::Left, 0.5, &[], 60), None);
    }

    #[test]
    fn what_is_still_held_is_released_and_nothing_else() {
        let mut h = Held::default();
        for e in [
            Event::Key { keycode: 42, pressed: true },  // Shift down...
            Event::Key { keycode: 30, pressed: true },  // A down
            Event::Key { keycode: 30, pressed: false }, // A up
            Event::Button { code: BTN_LEFT, pressed: true },
            Event::MouseRel { dx: 5.0, dy: 0.0 },
        ] {
            h.note(&e);
        }
        // ...and the source vanished. Shift and the button come up; A,
        // already up, is not sent again.
        assert_eq!(
            h.release_all(),
            vec![Event::Button { code: BTN_LEFT, pressed: false }, Event::Key { keycode: 42, pressed: false }]
        );
        assert!(h.release_all().is_empty(), "released once, then nothing");
    }

    #[test]
    fn the_size_line_round_trips_and_refuses_anything_else() {
        assert_eq!(parse_size_line(&size_line(2560, 1440)), Some((2560, 1440)));
        assert_eq!(parse_size_line("oxdesk-size 0 1440"), None);
        assert_eq!(parse_size_line("oxdesk-size 2560"), None);
        assert_eq!(parse_size_line("size 2560 1440"), None);
        assert_eq!(parse_size_line("oxdesk-size 2560 1440 9"), None);
    }

    fn rel(dx: f64, dy: f64) -> Event {
        Event::MouseRel { dx, dy }
    }

    #[test]
    fn without_a_size_or_an_edge_motion_stays_relative() {
        let mut p = Pointer::new(Some(Edge::Right), 0.5);
        assert_eq!(p.map(rel(5.0, 0.0)), Step::Send(rel(5.0, 0.0)));
        let mut p = Pointer::new(None, 0.5);
        assert_eq!(p.set_size(100, 100), None);
        assert_eq!(p.map(rel(5.0, 0.0)), Step::Send(rel(5.0, 0.0)));
    }

    #[test]
    fn it_enters_just_inside_the_edge_that_faces_home() {
        // This machine is to the RIGHT of the other one: the cursor appears
        // at the other screen's right edge, at the height it left at.
        let mut p = Pointer::new(Some(Edge::Right), 0.25);
        assert_eq!(p.set_size(2560, 1441), Some(Event::Mouse { x: 2539.0, y: 360.0 }));
        let mut p = Pointer::new(Some(Edge::Left), 1.0);
        assert_eq!(p.set_size(1920, 1081), Some(Event::Mouse { x: 20.0, y: 1080.0 }));
    }

    #[test]
    fn a_push_past_the_edge_brings_it_home_and_a_touch_does_not() {
        let mut p = Pointer::new(Some(Edge::Right), 0.5);
        p.set_size(1000, 501);
        // Reach the edge and a little past it: a scroll bar, not a return.
        assert!(matches!(p.map(rel(20.0, 0.0)), Step::Send(Event::Mouse { x, .. }) if x == 999.0));
        assert!(matches!(p.map(rel(30.0, 0.0)), Step::Send(_)));
        // Back in: the push starts over.
        assert!(matches!(p.map(rel(-1.0, 0.0)), Step::Send(_)));
        assert!(matches!(p.map(rel(1.0, 0.0)), Step::Send(_)));
        assert!(matches!(p.map(rel(30.0, 0.0)), Step::Send(_)));
        // Keep pushing: home, at the height it was at.
        assert_eq!(p.map(rel(20.0, 0.0)), Step::Return(0.5));
    }

    #[test]
    fn the_other_edges_do_not_bring_it_home() {
        let mut p = Pointer::new(Some(Edge::Right), 0.5);
        p.set_size(1000, 500);
        for _ in 0..20 {
            assert!(matches!(p.map(rel(-100.0, -100.0)), Step::Send(_)));
        }
        assert!(matches!(p.map(rel(0.0, 0.0)), Step::Send(Event::Mouse { x, y }) if x == 0.0 && y == 0.0));
    }

    #[test]
    fn keys_and_buttons_pass_through() {
        let mut p = Pointer::new(Some(Edge::Left), 0.5);
        p.set_size(100, 100);
        let k = Event::Key { keycode: 30, pressed: true };
        assert_eq!(p.map(k), Step::Send(k));
    }

    #[test]
    fn every_key_the_windows_host_types_comes_back_as_itself() {
        let mut n = 0;
        for k in 1..=127u32 {
            if let Some((scan, ext)) = scancode_from_evdev(k) {
                assert_eq!(evdev_from_scancode(scan, ext), Some(k), "scancode {scan:#x} ext={ext}");
                n += 1;
            }
        }
        // A loop over an empty table passes too: the count is the check.
        assert!(n > 100, "only {n} keys in the table");
    }

    #[test]
    fn the_extended_flag_decides_between_the_two_halves_of_the_keyboard() {
        assert_eq!(evdev_from_scancode(0x1D, false), Some(29)); // left Ctrl
        assert_eq!(evdev_from_scancode(0x1D, true), Some(97)); // right Ctrl
        assert_eq!(evdev_from_scancode(0x48, false), Some(72)); // keypad 8
        assert_eq!(evdev_from_scancode(0x48, true), Some(103)); // arrow up
        // A scancode no table entry produces is dropped, not guessed.
        assert_eq!(evdev_from_scancode(0x2A, true), None);
    }

    #[test]
    fn the_hook_codes_that_need_more_than_the_table() {
        // Pause and NumLock: same scancode, the virtual key decides.
        assert_eq!(evdev_from_windows_key(0x13, 0x45, false), Some(119));
        assert_eq!(evdev_from_windows_key(0x90, 0x45, true), Some(69));
        // The fake Ctrl in front of AltGr is dropped, not turned into a
        // left Ctrl (which is half of the default rescue chord).
        assert_eq!(evdev_from_windows_key(0xA2, 0x21D, false), None);
        // The ordinary path goes through the table.
        assert_eq!(evdev_from_windows_key(0xA3, 0x1D, true), Some(97)); // right Ctrl
        assert_eq!(evdev_from_windows_key(0x41, 0x1E, false), Some(30)); // A
    }

    #[test]
    fn a_cursor_put_back_on_an_edge_is_moved_off_it() {
        // Virtual desktop of two monitors, the left one at negative x.
        let rect = (-1920, 0, 4480, 1440);
        assert_eq!(inset_from_edges((-1920, 400), rect, 60), (-1860, 400));
        assert_eq!(inset_from_edges((2559, 1439), rect, 60), (2499, 1379));
        // Away from every edge it does not move.
        assert_eq!(inset_from_edges((100, 700), rect, 60), (100, 700));
        // A rectangle too small to inset leaves the point alone.
        assert_eq!(inset_from_edges((5, 5), (0, 0, 100, 100), 60), (5, 5));
    }

    #[test]
    fn a_rescue_needs_two_different_keys() {
        assert_eq!(parse_chord("29,97"), Some((29, 97)));
        assert_eq!(parse_chord(" 42 , 54 "), Some((42, 54)));
        assert_eq!(parse_chord("29,29"), None);
        assert_eq!(parse_chord("29"), None);
        assert_eq!(parse_chord("ctrl,ctrl"), None);
        assert_eq!(parse_chord("0,97"), None);
    }

    #[test]
    fn the_chord_fires_on_both_keys_and_not_on_one() {
        let set = |k: &[u32]| k.iter().copied().collect::<std::collections::HashSet<u32>>();
        assert!(chord_held(&set(&[42, 54]), (42, 54)));
        assert!(chord_held(&set(&[42, 54, 30]), (42, 54)));
        assert!(!chord_held(&set(&[42]), (42, 54)));
        assert!(!chord_held(&set(&[29, 97]), (42, 54)));
    }
}
