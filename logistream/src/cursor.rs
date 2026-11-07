//! Compositing the cursor onto the captured frame.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! It lives in its own module, without `cfg(windows)`, for a practical
//! reason: the logic is pixel arithmetic and depends on no platform at all,
//! and while it lived in `capture_win.rs` its tests **never ran** -- that
//! whole file is compiled only on Windows, and the suite runs on Linux. A test
//! that does not run is documentation dressed up as a guarantee.
//!
//! Same decision that moved `scancode_from_evdev` into `input.rs`.

// DXGI delivers the screen **without** the pointer: the position comes in
// `DXGI_OUTDUPL_FRAME_INFO` and the image in `GetFramePointerShape`,
// because that is how the hardware composes it.
//
// This did not matter with the viewer: it sends ABSOLUTE positions, so the
// viewer machine's own cursor sits exactly over the remote point -- with
// zero latency, even. With the KVM the trick falls apart: the local pointer
// is grabbed and still, and the remote one was never drawn. No cursor is
// left at all, and the person sees hover effects without knowing where
// they are.

/// Format of the pointer image, as DXGI classifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// 1 bpp, AND mask followed by XOR mask; height is doubled.
    Mono,
    /// 32 bpp BGRA with real alpha.
    Color,
    /// 32 bpp where alpha is a mask: 0 = paint, 0xFF = invert the background.
    MaskedColor,
}

/// A pointer image, as it came from DXGI.
#[derive(Debug, Clone)]
pub struct Shape {
    pub kind: ShapeKind,
    pub w: u32,
    pub h: u32,
    pub pitch: usize,
    pub bytes: Vec<u8>,
    /// Point of the image that sits under the pointer coordinate.
    pub hot: (i32, i32),
}

/// Rectangle covered by a shape drawn at (x, y), clipped to the screen.
///
/// Returns `None` when nothing visible is left -- a cursor on the edge is
/// partially cut, and a cursor off screen draws nothing. Kept apart from the
/// drawing because "save what was underneath" needs exactly the same
/// computation, and the two diverging would leave cursor trails on screen.
pub fn clip(
    screen_w: u32,
    screen_h: u32,
    s_w: u32,
    s_h: u32,
    x: i32,
    y: i32,
) -> Option<(u32, u32, u32, u32)> {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + s_w as i32).min(screen_w as i32);
    let y1 = (y + s_h as i32).min(screen_h as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Draws the shape at (x, y) over BGRA pixels.
///
/// `x`/`y` already account for the hotspot. A pure function over the
/// buffer: this is where the per-format decision lives, and this is what
/// the tests look at.
pub fn draw(px: &mut [u8], stride: usize, screen_w: u32, screen_h: u32, f: &Shape, x: i32, y: i32) {
    // Mono stores AND and XOR stacked, so the useful height is half.
    let height = if f.kind == ShapeKind::Mono { f.h / 2 } else { f.h };
    let Some((rx, ry, rw, rh)) = clip(screen_w, screen_h, f.w, height, x, y) else {
        return;
    };

    for ly in 0..rh {
        let fy = (ry + ly) as i32 - y;
        for lx in 0..rw {
            let fx = (rx + lx) as i32 - x;
            let dest = (ry + ly) as usize * stride + (rx + lx) as usize * 4;
            if dest + 3 >= px.len() {
                continue;
            }

            match f.kind {
                ShapeKind::Mono => {
                    // One bit per pixel, MSB first. `and` 0 paints the
                    // color; `and` 1 with `xor` 1 inverts what was
                    // underneath -- that is how the text cursor stays
                    // visible over both light and dark backgrounds.
                    let byte = fy as usize * f.pitch + (fx as usize) / 8;
                    let bit = 7 - (fx % 8);
                    let half = (height as usize) * f.pitch;
                    let Some(&a) = f.bytes.get(byte) else { continue };
                    let Some(&o) = f.bytes.get(half + byte) else { continue };
                    let a = (a >> bit) & 1;
                    let o = (o >> bit) & 1;
                    for c in 0..3 {
                        let bg = px[dest + c];
                        px[dest + c] = match (a, o) {
                            (0, 0) => 0x00,
                            (0, 1) => 0xFF,
                            (1, 0) => bg,
                            _ => !bg,
                        };
                    }
                }
                ShapeKind::Color => {
                    let o = fy as usize * f.pitch + fx as usize * 4;
                    if o + 3 >= f.bytes.len() {
                        continue;
                    }
                    let a = f.bytes[o + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    // Plain alpha. No gamma on purpose: the cursor has
                    // near-binary alpha at its edges and the difference
                    // does not pay for the per-pixel cost.
                    for c in 0..3 {
                        let fg = f.bytes[o + c] as u32;
                        let bg = px[dest + c] as u32;
                        px[dest + c] = ((fg * a + bg * (255 - a)) / 255) as u8;
                    }
                }
                ShapeKind::MaskedColor => {
                    let o = fy as usize * f.pitch + fx as usize * 4;
                    if o + 3 >= f.bytes.len() {
                        continue;
                    }
                    // Here the "alpha" channel is not transparency: 0 means
                    // copy, 0xFF means invert the background.
                    let mask = f.bytes[o + 3];
                    for c in 0..3 {
                        px[dest + c] = if mask == 0 {
                            f.bytes[o + c]
                        } else {
                            px[dest + c] ^ f.bytes[o + c]
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    fn screen(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
    }

    #[test]
    fn off_screen_draws_nothing() {
        assert_eq!(clip(100, 100, 32, 32, -40, 10), None);
        assert_eq!(clip(100, 100, 32, 32, 10, 200), None);
    }

    #[test]
    fn on_the_edge_draws_only_the_visible_part() {
        // Cursor half past the right edge: 10 columns remain.
        assert_eq!(clip(100, 100, 32, 32, 90, 50), Some((90, 50, 10, 32)));
        // And to the left, the origin moves with it.
        assert_eq!(clip(100, 100, 32, 32, -10, 50), Some((0, 50, 22, 32)));
    }

    #[test]
    fn color_with_full_alpha_replaces_the_background() {
        let mut px = screen(4, 4);
        let f = Shape {
            kind: ShapeKind::Color,
            w: 1,
            h: 1,
            pitch: 4,
            bytes: vec![10, 20, 30, 255],
            hot: (0, 0),
        };
        draw(&mut px, 16, 4, 4, &f, 1, 1);
        let o = 1 * 16 + 1 * 4;
        assert_eq!(&px[o..o + 3], &[10, 20, 30]);
    }

    #[test]
    fn color_with_zero_alpha_leaves_the_background_alone() {
        let mut px = screen(4, 4);
        px[0] = 99;
        let f = Shape {
            kind: ShapeKind::Color,
            w: 1,
            h: 1,
            pitch: 4,
            bytes: vec![10, 20, 30, 0],
            hot: (0, 0),
        };
        draw(&mut px, 16, 4, 4, &f, 0, 0);
        assert_eq!(px[0], 99, "zero alpha must leave the background intact");
    }

    #[test]
    fn mono_paints_black_white_and_inverts() {
        // One row of 8 pixels. AND=0b0011_1111 XOR=0b0101_1111:
        //   pixel 0: and=0 xor=0 -> black
        //   pixel 1: and=0 xor=1 -> white
        //   pixel 2: and=1 xor=0 -> keeps the background
        //   pixel 3: and=1 xor=1 -> inverts the background
        let mut px = screen(8, 2);
        for i in 0..8 {
            px[i * 4] = 100;
            px[i * 4 + 1] = 100;
            px[i * 4 + 2] = 100;
        }
        let f = Shape {
            kind: ShapeKind::Mono,
            w: 8,
            h: 2, // AND + XOR stacked: useful height = 1
            pitch: 1,
            bytes: vec![0b0011_1111, 0b0101_1111],
            hot: (0, 0),
        };
        draw(&mut px, 32, 8, 2, &f, 0, 0);
        assert_eq!(px[0], 0x00, "and=0 xor=0 -> black");
        assert_eq!(px[4], 0xFF, "and=0 xor=1 -> white");
        assert_eq!(px[8], 100, "and=1 xor=0 -> background intact");
        assert_eq!(px[12], 155, "and=1 xor=1 -> background inverted");
    }

    #[test]
    fn masked_copies_or_inverts() {
        let mut px = screen(2, 1);
        px[0] = 100;
        px[4] = 100;
        let f = Shape {
            kind: ShapeKind::MaskedColor,
            w: 2,
            h: 1,
            pitch: 8,
            bytes: vec![7, 7, 7, 0x00, 0xFF, 0, 0, 0xFF],
            hot: (0, 0),
        };
        draw(&mut px, 8, 2, 1, &f, 0, 0);
        assert_eq!(px[0], 7, "mask 0 copies the color");
        assert_eq!(px[4], 100 ^ 0xFF, "mask 0xFF inverts the background");
    }
}

// --- our own cursor ---------------------------------------------------
//
// Windows **hides the cursor when the machine has no mouse**, and under the
// KVM it never has one: the dongle is on the other side. Measured with
// `GetCursorInfo`: `flags=0` (hidden) and `hCursor=0` (no cursor exists),
// with the correct position -- which is why hover responded and nothing
// showed up.
//
// In that situation asking DXGI for the image does not help: it reports
// `Visible=false` and never sends any shape, because indeed nothing is
// being drawn. We are the ones who have to draw it.

/// Default arrow, 12x19, in the mask format of the monochrome cursor.
///
/// Drawn by hand instead of asked of the system (`LoadCursorW` +
/// `GetIconInfo` + `GetDIBits`) because here it is a FALLBACK: it only kicks
/// in when the system has no cursor to give. Asking someone who has none
/// would be circular, and the Win32 path to extract the bitmap has more
/// chances of failing than these nineteen lines.
///
/// `K` black, `W` white, `.` transparent. The white outline exists so the
/// arrow does not vanish over a dark background -- the same reason the
/// system cursor has one.
const ARROW: [&str; 19] = [
    "W...........",
    "WW..........",
    "WKW.........",
    "WKKW........",
    "WKKKW.......",
    "WKKKKW......",
    "WKKKKKW.....",
    "WKKKKKKW....",
    "WKKKKKKKW...",
    "WKKKKKKKKW..",
    "WKKKKKKKKKW.",
    "WKKKKKKWWWWW",
    "WKKKWKKW....",
    "WKKWWKKW....",
    "WKW..WKKW...",
    "WW...WKKW...",
    "W.....WKKW..",
    "......WKKW..",
    ".......WWW..",
];

/// Builds the built-in arrow as a monochrome `Shape`.
///
/// Reuses the `Mono` path of `draw`, which already handles AND/XOR -- a
/// second drawing path just for this case would be more code for the same
/// picture.
pub fn builtin_arrow() -> Shape {
    let w = ARROW[0].len() as u32;
    let h = ARROW.len() as u32;
    let pitch = w.div_ceil(8) as usize;
    // AND and XOR stacked, in this order, as DXGI delivers them.
    let mut bytes = vec![0u8; pitch * h as usize * 2];
    let half = pitch * h as usize;

    for (y, row) in ARROW.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            let byte = y * pitch + x / 8;
            let bit = 7 - (x % 8);
            match c {
                // transparent: and=1, xor=0 -> keeps the background
                b'.' => bytes[byte] |= 1 << bit,
                // black: and=0, xor=0
                b'K' => {}
                // white: and=0, xor=1
                _ => bytes[half + byte] |= 1 << bit,
            }
        }
    }

    Shape { kind: ShapeKind::Mono, w, h: h * 2, pitch, bytes, hot: (0, 0) }
}

#[cfg(test)]
mod arrow_tests {
    use super::*;

    #[test]
    fn the_arrow_tip_is_white_and_the_body_black() {
        let f = builtin_arrow();
        // Gray background to tell "painted" from "let through".
        let mut px = vec![100u8; 32 * 32 * 4];
        draw(&mut px, 32 * 4, 32, 32, &f, 0, 0);

        let at = |x: usize, y: usize| px[y * 32 * 4 + x * 4];
        assert_eq!(at(0, 0), 0xFF, "the tip is the white outline");
        assert_eq!(at(1, 2), 0x00, "the body is black");
        assert_eq!(at(11, 0), 100, "outside the arrow the background stays intact");
    }

    #[test]
    fn the_arrow_fits_its_own_buffer() {
        let f = builtin_arrow();
        let height = f.h / 2;
        assert_eq!(f.bytes.len(), f.pitch * height as usize * 2);
        assert_eq!(height, 19);
        assert_eq!(f.w, 12);
    }

    /// Drawing on the edge must not overflow the buffer.
    #[test]
    fn on_the_edge_does_not_write_outside() {
        let f = builtin_arrow();
        let mut px = vec![0u8; 16 * 16 * 4];
        // Four corners, all with the arrow partially outside.
        for (x, y) in [(-5, -5), (14, 14), (-5, 14), (14, -5)] {
            draw(&mut px, 16 * 4, 16, 16, &f, x, y);
        }
    }
}
