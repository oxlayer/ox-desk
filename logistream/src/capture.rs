//! Where the frame comes from.
//!
//! Linux/Wayland: `ScreenCast` portal + PipeWire. There is no shortcut -- the
//! compositor lets no one read the screen without going through it, which is
//! the same reason the edge talks to `Mutter.InputCapture` instead of reading
//! the cursor.
//!
//! Windows: DXGI Desktop Duplication.

/// A raw frame, as the platform delivered it.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Nanoseconds of the monotonic clock. Without this there is no way to
    /// measure end-to-end latency, and without measuring there is no way to
    /// improve it.
    pub captured_at_ns: u64,
    /// Linear BGRA, or a texture handle once capture is zero-copy. Starting
    /// simple.
    pub data: Vec<u8>,
}

/// Frame source. A trait because there are three implementations and they
/// share nothing beyond this shape.
pub trait Capture: Send {
    fn next_frame(&mut self) -> anyhow::Result<Frame>;
    fn size(&self) -> (u32, u32);
}
