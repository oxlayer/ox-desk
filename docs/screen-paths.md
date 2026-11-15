# The screen paths

Five combinations of who watches and who is watched. Since NVDEC
(2026-11-04) **all of them are ours**: none goes through RDP or VNC. The
Mac as host still captures through FFmpeg's `avfoundation`, which
[`specs/020`](../specs/020-native-mac.md) replaces with ScreenCaptureKit.

| From (watches) | To (host) | Capture | Encode | Decode |
|---|---|---|---|---|
| Ubuntu | Windows | DXGI Desktop Duplication | NVENC | NVDEC |
| Windows | Ubuntu | PipeWire via Mutter ScreenCast | NVENC | NVDEC |
| Mac | Windows | DXGI | NVENC | ffmpeg (`swscale`) |
| Mac | Ubuntu | PipeWire | NVENC | ffmpeg |
| Ubuntu/Windows | Mac | avfoundation | VideoToolbox | NVDEC / ffmpeg |

The Mac host starts through a **LaunchAgent**, not over SSH: capture
depends on a TCC permission granted to whoever launched it, and over SSH
AVFoundation hangs instead of failing
([[../.claude/rules/graphical-session]]).

The Mac decodes with ffmpeg because it has no NVDEC. That stopped being a
detail when the Mac became the **control group**: the Mac path used
`swscale` (bilinear) and looked great while the GPU path, with
nearest-neighbor sampling, jagged every letter. Comparing the two is what
pointed to the bug — see [[../specs/010-remote-desktop.md]].

## Transport

QUIC (`quinn`): video by **datagram** (losing a frame is better than
delaying), input by **stream** (it can't lose a key or reorder).

The host serves **one client at a time**. A second `oxdesk-sink` trying
to measure while someone is watching times out. That is not a bug; it is
the accept queue.

## Input

The protocol carries **evdev** keycodes on every path, because that is
what Mutter's `NotifyKeyboardKeycode` receives; `scancode_from_evdev()`
translates to Set-1 on Windows.

Two kinds of pointer movement:

- `Mouse { x, y }` — absolute, in stream space. The viewer uses this one:
  it knows where the cursor is inside the window.
- `MouseRel { dx, dy }` — relative. The LAN KVM uses this one, because
  games read relative movement and with absolute the aim gets stuck at
  the edges.

## See also

- [[new-computer]] — the order of operations on a machine
- [[../specs/010-remote-desktop.md]] — the full stack
- [[../specs/005-input-switch.md]] — machine switching and KVM

## Ubuntu with no monitor on

With the outputs `disconnected`, `RecordMonitor` answers `Unknown
monitor`. The host then asks for `RecordVirtual`, at 1920×1080, and GNOME
creates the monitor `Meta-0` for as long as the host lives.

The virtual stream renegotiates the format right after it is born, with
the same size. Swapping the encoder at that moment caused SIGSEGV
(exit 139), so the host keeps the one it has.

Measured on 2026-11-07: 25 frames in 10 s, zero lost, with the screen
still. Mutter only delivers a frame when something changes.

## Windows with no monitor on

With no monitor DXGI has no output: `DuplicateOutput` refuses with
`0x80070057` and the client connects without receiving frames. Windows
has no native virtual screen, so we use the Virtual Display Driver,
signed by the SignPath Foundation (`oxdesk/setup/install-virtual-display.ps1`).

The script puts the publisher in `TrustedPublisher` only during the
installation, because the confirmation prompt doesn't show up over SSH,
and removes it afterwards. Apart from that, nothing there asks to trust a
self-signed certificate.

Measured on 2026-11-07, from Ubuntu over the tailnet: 243 frames in
10 s, zero lost.

The driver starts at 800×600, and that is what exposed the row-stride
bug in `nvenc.rs`: the frame arrived scrambled horizontally, with a
black band at the bottom, and the `sink` counted 243 perfect frames.
Counting frames doesn't prove the image: decode one with `ffmpeg` and
look at it.

The virtual screen starts at 30 Hz, and that held the stream at
~24 fps. The installer asks for 60 Hz.
