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

/// Scroll pixels in one line of text -- the unit `Event::Scroll` carries.
///
/// It lives here, with the protocol, because it is a **wire** unit: every
/// end has to agree on it, and until 08/11 two of them did not. The viewer
/// sends `LineDelta * 15` and the Windows injector divides by 15; the LAN
/// KVM sent 120 px per notch -- Windows' own wheel unit -- so one click of
/// the wheel scrolled EIGHT lines there. Measured on the windows-host machine.
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
