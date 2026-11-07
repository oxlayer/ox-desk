//! Capture on Windows: DXGI Desktop Duplication.
//!
//! Spec: ../specs/010-remote-desktop.md
//!
//! ## Why DXGI and not GDI
//!
//! GDI's `BitBlt` copies the screen through the CPU, does not see
//! accelerated windows and is expensive at 1440p. Desktop Duplication hands
//! over the texture the compositor **has already assembled**, on the GPU,
//! and tells us when nothing changed -- which means zero work with a still
//! screen. It is the same path Parsec and OBS use.
//!
//! ## The cursor does not come in the image
//!
//! This is the visible difference from the Linux host, where we ask Mutter
//! for `cursor-mode: 1` and it embeds the cursor in the frame. DXGI delivers
//! the screen **without** the cursor and the pointer separately
//! (`GetFramePointerShape`), because that is how the hardware composes it.
//!
//! We do not draw the cursor for now, and that is less serious than it
//! looks: the client sends ABSOLUTE positions, so your own machine's cursor
//! sits exactly over the remote point -- with zero latency, even. What is
//! missing is the SHAPE (the text bar, the resize arrows). Parsec does the
//! same for this reason.
//!
//! ## The copy to the CPU is deliberate
//!
//! The right thing would be to register the D3D11 texture directly with
//! NVENC (`NV_ENC_DEVICE_TYPE_DIRECTX`) and never leave the GPU. We take the
//! detour through the CPU -- staging texture, `Map`, upload to CUDA -- for
//! one reason: it is EXACTLY the path the Linux host already takes
//! (PipeWire delivers in CPU memory), so there is a single encoder. Changing
//! this is an optimization local to this file, not an architecture change.

#![cfg(windows)]

use crate::cursor::{draw, clip, Shape, ShapeKind};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ_WRITE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
};

/// What came out of an attempt to grab a frame.
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureState {
    /// New image in the staging texture; it can be mapped and encoded.
    New,
    /// Nothing changed within the timeout. A still screen is the common
    /// case, not an error.
    Idle,
    /// The duplication died -- resolution change, UAC screen, a fullscreen
    /// game taking over the display. It has to be recreated.
    Lost,
}

/// Pixels mapped from the staging texture. Unmaps when it goes out of scope.
///
/// A guard and not `(pointer, size)` because forgetting the `Unmap` locks
/// the texture forever and the next `CopyResource` fails silently.
pub struct Mapped<'a> {
    context: &'a ID3D11DeviceContext,
    resource: ID3D11Resource,
    pub px: &'a [u8],
    pub stride: usize,
}

impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        unsafe { self.context.Unmap(&self.resource, 0) };
    }
}

/// The monitor, before duplication starts.
///
/// Separate from `Screen` for a measured reason: holding the DXGI
/// duplication while CUDA initialized **hung the process**. The log stopped
/// at "DXGI: monitor 0, 2560x1440" with 0.2 s of CPU and stayed there -- the
/// same NVENC probe, on its own, ran at 111 fps in the same session. So the
/// order became: open the monitor, read the size, build the encoder, and
/// ONLY THEN duplicate.
///
/// As a bonus the new order is a better design: if NVENC is not available,
/// we fail before taking over someone's screen.
pub struct Monitor {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    w: u32,
    h: u32,
    /// Corner of the monitor on the virtual desktop. Input needs it to
    /// convert a stream pixel into an absolute position on the machine.
    x: i32,
    y: i32,
}

impl Monitor {
    /// Opens monitor `index` (0 = primary) without starting to capture.
    pub fn open(index: u32) -> anyhow::Result<Self> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                // BGRA_SUPPORT: the desktop is BGRA, and without the flag
                // creating the staging texture may be refused for the format.
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice returned no device"))?;
            let context = context
                .ok_or_else(|| anyhow::anyhow!("D3D11CreateDevice returned no context"))?;

            // From the device to the adapter, from the adapter to the
            // output: the output must be duplicated from the SAME adapter
            // that created the device, otherwise the texture would cross
            // cards.
            let dxgi: IDXGIDevice = device.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let output = adapter
                .EnumOutputs(index)
                .map_err(|e| anyhow::anyhow!("monitor {index}: {e}"))?;
            let output: IDXGIOutput1 = output.cast()?;
            let desc = output.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            println!("  DXGI: monitor {index}, {w}x{h}");
            Ok(Monitor { device, context, output, w, h, x: r.left, y: r.top })
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    /// Top-left corner of the monitor within the virtual desktop.
    pub fn origin(&self) -> (i32, i32) {
        (self.x, self.y)
    }

    /// Starts capturing. From here on the compositor waits for us on every
    /// frame, so everything slow has to be ready.
    ///
    /// Tries a few times before giving up: `DuplicateOutput` refuses for
    /// TEMPORARY reasons (UAC screen in front, session switching), and dying
    /// on the first one would make the "Ver a tela" button fail because of a
    /// dialog that goes away on its own in two seconds.
    pub fn duplicate(self) -> anyhow::Result<Screen> {
        const ATTEMPTS: u32 = 5;
        let mut last = String::new();
        for i in 0..ATTEMPTS {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(600));
            }
            match unsafe { self.output.DuplicateOutput(&self.device) } {
                Ok(dup) => {
                    let staging = create_staging(&self.device, self.w, self.h)?;
                    return Ok(Screen {
                        m: self,
                        dup,
                        staging,
                        has_image: false,
                        shape: None,
                        pointer: (0, 0),
                        visible: false,
                        saved: None,
                        drew_once: false,
                        system_visible: false,
                    });
                }
                Err(e) => {
                    last = e.to_string();
                    println!("  DuplicateOutput refused ({}/{ATTEMPTS}): {e}", i + 1);
                }
            }
        }
        // "Access denied" on its own sends the person looking for user
        // permissions, which is NEVER the problem. There are four causes
        // with the same code, and I diagnosed the wrong one three times in
        // one day -- so the order here is by MEASURED frequency, not by
        // intuition:
        //
        // 1. Screensaver running. It lives on its own desktop and DXGI does
        //    not duplicate for anyone not on the input desktop. It was the
        //    cause of "I can't wake Windows up", with LogonUI absent and
        //    the video adapter OK -- the two signals that made me discard
        //    the other hypotheses. The host already tries to end it at
        //    startup.
        // 2. Screen actually locked (LogonUI in front).
        // 3. UAC dialog on the secure desktop.
        // 4. Half-installed display stack -- it was real when Parsec's
        //    virtual driver was removed and left a `Microsoft Basic
        //    Display Adapter` in `Unknown` state with a pending reboot.
        //
        // The playbook for telling them apart is in
        // .claude/rules/windows-locked-screen.md.
        anyhow::bail!(
            "DuplicateOutput refused {ATTEMPTS} times ({last}). \
             Four causes give this same error; check in this order: \
             (1) screensaver running -- `Get-Process | ? Path -like '*.scr'`; \
             (2) screen locked -- `Get-Process LogonUI`; \
             (3) UAC dialog in front -- `Get-Process consent`; \
             (4) half-installed video driver -- `Get-CimInstance Win32_VideoController`. \
             DXGI does not capture any desktop other than the input one, by design."
        )
    }
}

pub struct Screen {
    m: Monitor,
    dup: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    /// There was at least one frame: without this, mapping would read garbage.
    has_image: bool,
    /// Last pointer image DXGI sent. It only resends when the shape
    /// CHANGES, so keeping it is mandatory -- without this the cursor would
    /// appear in one frame and vanish in the next.
    shape: Option<Shape>,
    pointer: (i32, i32),
    visible: bool,
    /// What was under the cursor before we drew it, and where.
    ///
    /// We compose the cursor INSIDE the staging texture, which is not
    /// re-copied when only the pointer moves. Without undoing the previous
    /// drawing, every movement would leave a cursor stuck behind -- a trail
    /// of arrows across the screen.
    saved: Option<(u32, u32, u32, u32, Vec<u8>)>,
    /// Only so the log says once that compositing is up.
    drew_once: bool,
    /// What DXGI says about the SYSTEM cursor being visible.
    system_visible: bool,
}

impl Screen {
    pub fn size(&self) -> (u32, u32) {
        self.m.size()
    }

    pub fn origin(&self) -> (i32, i32) {
        self.m.origin()
    }

    pub fn has_image(&self) -> bool {
        self.has_image
    }

    /// Tries to grab a new frame, waiting at most `wait_ms`.
    ///
    /// On `New`, the image stays in the staging texture and comes out
    /// through `map_frame`. The split exists so that `ReleaseFrame` happens
    /// **before** `Map`: until the frame is handed back, the compositor
    /// waits for us.
    pub fn next_frame(&mut self, wait_ms: u32) -> anyhow::Result<CaptureState> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(wait_ms, &mut info, &mut resource) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(CaptureState::Idle),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Ok(CaptureState::Lost),
                Err(e) => return Err(anyhow::anyhow!("AcquireNextFrame: {e}")),
            }

            // Raw diagnostics of the first frames: the only way to know
            // what DXGI really reports about the pointer.
            // Position from `GetCursorPos`, not from DXGI.
            //
            // DXGI only reports a pointer when one is being DRAWN. On this
            // machine there is none: it has no physical mouse -- the dongle
            // is on the other side --, and Windows hides the cursor when
            // there is no pointing device. Measured with `GetCursorInfo`:
            // flags=0, hCursor=0, and still the correct position. That is
            // why hover responded and nothing showed up.
            //
            // `GetCursorPos` returns the position regardless, visible or
            // not, which is exactly what we need.
            let mut pt = POINT::default();
            let moved = if GetCursorPos(&mut pt).is_ok() {
                let new = (pt.x, pt.y);
                let changed = new != self.pointer;
                self.pointer = new;
                changed
            } else {
                false
            };
            // Visibility: only believe DXGI when it has a real cursor to
            // talk about.
            //
            // With no mouse on the machine, `Visible` stays false forever,
            // and obeying it would erase the built-in arrow -- which exists
            // precisely for that case. With a mouse, `Visible` starts to
            // mean something: a fullscreen game hides the pointer on
            // purpose, and drawing one over it would be us intruding.
            if info.LastMouseUpdateTime != 0 {
                self.system_visible = info.PointerPosition.Visible.as_bool();
            }
            self.visible = self.shape.is_none() || self.system_visible;

            // If the system HAS a cursor, it sends the shape when it
            // changes, and drawing its shape is better than ours (hand, text
            // bar, resize). When nothing ever comes, we use the built-in
            // arrow -- see `cursor::builtin_arrow`.
            if info.PointerShapeBufferSize > 0 {
                self.read_shape(info.PointerShapeBufferSize);
            }

            // `LastPresentTime == 0` means only the pointer moved -- the
            // desktop image is the same.
            let pointer_only = info.LastPresentTime == 0;

            // But "only the pointer" is no longer a reason to send nothing:
            // since we draw the cursor into the frame, moving the mouse
            // CHANGES the image. In a KVM that is the main thing: the person
            // looks at the remote screen to know where the cursor is. The
            // re-encode is cheap -- the scene is identical outside the
            // cursor, and a P-frame of a few bytes comes out.
            let state = if !pointer_only || (moved && self.has_image) {
                CaptureState::New
            } else {
                CaptureState::Idle
            };

            if !pointer_only {
                if let Some(r) = &resource {
                    let tex: ID3D11Texture2D = r.cast()?;
                    self.m.context.CopyResource(&self.staging, &tex);
                    self.has_image = true;
                    // The new copy erased the cursor we had drawn, so there
                    // is nothing to undo.
                    self.saved = None;
                }
            }
            // Always: handing the frame back is mandatory, even when only
            // the pointer changed. Not handing it back stalls capture on the
            // next Acquire with "a frame is already open".
            let _ = self.dup.ReleaseFrame();
            Ok(state)
        }
    }

    /// Reads the pointer image DXGI just announced.
    fn read_shape(&mut self, size: u32) {
        let mut buf = vec![0u8; size as usize];
        let mut used = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        let r = unsafe {
            self.dup.GetFramePointerShape(
                size,
                buf.as_mut_ptr() as *mut _,
                &mut used,
                &mut info,
            )
        };
        if r.is_err() {
            return;
        }
        let kind = match info.Type {
            1 => ShapeKind::Mono,
            2 => ShapeKind::Color,
            4 => ShapeKind::MaskedColor,
            // A type we do not know: better NO cursor than a rectangle of
            // garbage in the middle of someone's screen.
            other => {
                eprintln!("  unknown pointer shape: {other}");
                self.shape = None;
                return;
            }
        };
        buf.truncate(used as usize);
        println!(
            "  pointer shape: {:?} {}x{} pitch={} ({} bytes)",
            kind, info.Width, info.Height, info.Pitch, used
        );
        self.shape = Some(Shape {
            kind,
            w: info.Width,
            h: info.Height,
            pitch: info.Pitch as usize,
            bytes: buf,
            hot: (info.HotSpot.x, info.HotSpot.y),
        });
    }

    /// Maps the last copied image for reading by the CPU.
    ///
    /// Serves both the frame that just arrived and resending the last one
    /// with a still screen -- the staging texture keeps the image, so
    /// resending costs no new copy.
    pub fn map_frame(&mut self) -> anyhow::Result<Mapped<'_>> {
        unsafe {
            let resource: ID3D11Resource = self.staging.cast()?;
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.m.context.Map(&resource, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut m))?;
            let stride = m.RowPitch as usize;
            let (w, h) = (self.m.w, self.m.h);
            let total = stride * h as usize;
            let mut_px = std::slice::from_raw_parts_mut(m.pData as *mut u8, total);

            // 1. Undo the previous cursor. It only exists when no new copy
            //    went over it.
            if let Some((rx, ry, rw, rh, data)) = self.saved.take() {
                for ly in 0..rh as usize {
                    let o = (ry as usize + ly) * stride + rx as usize * 4;
                    let width = rw as usize * 4;
                    let src = ly * width;
                    if o + width <= total && src + width <= data.len() {
                        mut_px[o..o + width].copy_from_slice(&data[src..src + width]);
                    }
                }
            }

            // 2. Save what is under the new cursor and draw it.
            if self.visible {
                let builtin;
                let shape = match &self.shape {
                    Some(f) => f,
                    None => {
                        builtin = crate::cursor::builtin_arrow();
                        &builtin
                    }
                };
                {
                    let f = shape;
                    let height = if f.kind == ShapeKind::Mono { f.h / 2 } else { f.h };
                    // Subtracting the hotspot here is RIGHT because the
                    // position comes from `GetCursorPos`, which returns the
                    // hot point (the tip of the arrow).
                    //
                    // It does not hold for the DXGI position: the
                    // `DXGI_OUTDUPL_POINTER_SHAPE_INFO` documentation says
                    // the app should put the top-left corner in
                    // `PointerPosition.Position` and **not** use the hotspot
                    // there. Whoever changes the source of the position has
                    // to remove these two lines with it, otherwise the
                    // cursor draws offset by the size of the hotspot.
                    let x = self.pointer.0 - f.hot.0;
                    let y = self.pointer.1 - f.hot.1;
                    if let Some((rx, ry, rw, rh)) = clip(w, h, f.w, height, x, y) {
                        let width = rw as usize * 4;
                        let mut data = Vec::with_capacity(width * rh as usize);
                        for ly in 0..rh as usize {
                            let o = (ry as usize + ly) * stride + rx as usize * 4;
                            if o + width <= total {
                                data.extend_from_slice(&mut_px[o..o + width]);
                            }
                        }
                        self.saved = Some((rx, ry, rw, rh, data));
                        draw(mut_px, stride, w, h, f, x, y);
                        if !self.drew_once {
                            self.drew_once = true;
                            println!("  cursor drawn at {x},{y} ({rw}x{rh})");
                        }
                    }
                }
            }

            let px = std::slice::from_raw_parts(m.pData as *const u8, total);
            Ok(Mapped { context: &self.m.context, resource, px, stride })
        }
    }

    /// Recreates the duplication after `Lost`.
    ///
    /// The resolution may have changed along with it (that is what brought
    /// the previous one down), so the staging texture and the size are
    /// rebuilt too -- returns `true` when the caller needs to rebuild the
    /// encoder.
    pub fn recreate(&mut self) -> anyhow::Result<bool> {
        unsafe {
            let desc = self.m.output.GetDesc()?;
            let r = desc.DesktopCoordinates;
            let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
            self.dup = self
                .m
                .output
                .DuplicateOutput(&self.m.device)
                .map_err(|e| anyhow::anyhow!("DuplicateOutput: {e}"))?;
            // The origin can change without the size changing (monitor
            // dragged to the other side in the control panel), so it is
            // updated outside the `if`.
            self.m.x = r.left;
            self.m.y = r.top;
            let changed = (w, h) != (self.m.w, self.m.h);
            if changed {
                self.staging = create_staging(&self.m.device, w, h)?;
                self.m.w = w;
                self.m.h = h;
                self.has_image = false;
                println!("  DXGI: resolution changed to {w}x{h}");
            }
            Ok(changed)
        }
    }
}

/// Texture the CPU can read. `STAGING` + `CPU_ACCESS_READ` is the only
/// combination D3D11 allows to be mapped for reading.
fn create_staging(d: &ID3D11Device, w: u32, h: u32) -> anyhow::Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32,
        MiscFlags: 0,
    };
    let mut t: Option<ID3D11Texture2D> = None;
    unsafe { d.CreateTexture2D(&desc, None, Some(&mut t))? };
    t.ok_or_else(|| anyhow::anyhow!("CreateTexture2D returned no texture"))
}
