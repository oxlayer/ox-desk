# Capture pixel ≠ injection point

The protocol carries `Evento::Mouse` in **frame pixels**: the viewer
knows the size of what it receives and maps the window onto it
(`remote_point`, in `stream/src/bin/view.rs`). The host, on the
other side, injects in the **system's** coordinate space — and the two
are not the same space on every machine.

| System | Capture | Injection | Factor |
|---|---|---|---|
| macOS Retina | mode pixels (3360×2100) | CGEvent points (1680×1050) | **2** |
| Windows | physical pixels (DXGI) | `GetSystemMetrics`, logical if the process doesn't declare DPI | display scale |
| Linux | pixels | pixels (Mutter) | 1 |

## macOS: it was broken, it is fixed

`CGEventCreateMouseEvent` works in points. Sending the pixel directly
takes the pointer to twice the position: pointing at the middle of the
window throws the cursor to the bottom-right corner, because double of
everything is larger than the screen and the system clamps at the edge.
It arrived as *"the cursor position on the mac didn't match"*.

The conversion is `point_from_pixel` in `stream/src/input.rs`
(platform-neutral, with tests), and the scale comes from the **video
mode**:

```rust
CGDisplayModeGetPixelWidth(modo) / CGDisplayModeGetWidth(modo)   // 2.0
```

**`CGDisplayPixelsWide` doesn't work for this.** Despite the name, it
returns **points** — measured: 1680 on a display whose mode declares
3360 pixels. The name is what let the bug through: the edge clamp of
relative movement uses this function and is therefore right *by
accident*, and only the absolute path broke.

The host states the scale in the log at startup, because this symptom
can't be diagnosed without it:

```
  2 pixel(s) de captura por ponto de ponteiro
```

## Windows: latent, not active

`GetSystemMetrics` returns **logical** coordinates in a process that
doesn't declare DPI awareness — and a Rust binary without a manifest is
DPI-unaware. DXGI captures in physical. Outside 100% scale,
`input_win::move_to` would be dividing physical by logical and be off by
the scale.

Measured on 2026-11-04 with `windows/probes/read-cursor.ps1` (`/IT` task,
because over SSH you read session 0 and the number is invalid):

```
sem-dpi-aware: cursor=1258,1369 virtual=2560x1440
com-dpi-aware: cursor=1258,1369 virtual=2560x1440
```

Identical ⇒ the effective scale is 100% and the path is correct today.
(The `DpiValue=0x1` in `HKCU\Control Panel\Desktop\PerMonitorSettings`
belongs to a monitor that is not in use — the registry suggested scaling
and the measurement disproved it.)

**If the Windows scale changes, this breaks.** The fix is to declare
`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at host startup,
which makes `GetSystemMetrics` return physical and makes the code's
assumption true. It wasn't done now so as not to change, without being
able to test it, a path that is working.

## Rule

New host, or a new screen on an existing host: **measure both spaces
before trusting they are one**. The test is `--abs` in `input-send`, and
the case that reveals it is the **middle** of the frame — not the
corner, which the clamp disguises, and not relative movement, which
doesn't even go through the conversion.

```bash
oxdesk-input-send <host>:<porta+1> --abs 1680 1050   # middle of the frame
```

## See also

- [[../skills/verify-the-mac]] — the recipe with the four points
- [[cursor-without-mouse]] — the other cursor problem, unrelated to this one
- [[verify-what-arrived]] — why `--rel` saying "exactly +200" proved
  nothing here
