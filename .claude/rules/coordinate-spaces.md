# Capture pixel ≠ injection point

The protocol carries `Event::Mouse` in **frame pixels**: the viewer
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
Reported symptom: the cursor position on the Mac did not match.

The conversion is `point_from_pixel` in `stream/src/input.rs`
(platform-neutral, with tests), and the scale comes from the **video
mode**:

```rust
CGDisplayModeGetPixelWidth(mode) / CGDisplayModeGetWidth(mode)   // 2.0
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

That is the line as logged then: "2 capture pixel(s) per pointer point",
which is also what current builds print.

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

(`sem-dpi-aware` is "without DPI awareness", `com-dpi-aware` "with DPI
awareness".)

Identical ⇒ the effective scale is 100% and the path is correct today.
(The `DpiValue=0x1` in `HKCU\Control Panel\Desktop\PerMonitorSettings`
belongs to a monitor that is not in use — the registry suggested scaling
and the measurement disproved it.)

**If the Windows scale changes, this breaks.** The fix is to declare
`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` at host startup,
which makes `GetSystemMetrics` return physical and makes the code's
assumption true. It wasn't done now so as not to change, without being
able to test it, a path that is working.

## The wheel: the same mistake, in another unit (08/11)

Position was not the only number crossing the wire with two conventions.
`Event::Scroll` carries **pixels**, 15 to a line of text -- that is what
the viewer sends (`LineDelta * 15`) and what `input_win` divides by before
converting into Windows notches. The LAN KVM sent **120** px per notch,
because 120 is the number Windows uses for its own wheel unit. The
arithmetic on the other side came out `120/15*120 = 960`, or **eight
notches per physical click**. On a wheel that free-spins, one flick sent
the page to the end.

Reported symptom: the mouse misbehaved, yet movement and clicks were
fine -- which is what made it hard to name. The host log settled it:
every `MouseRel` arrived and the cursor overlay drew. The wheel was the
only broken thing.

Two things let it through, and both are already written above:

- **The unit had no owner.** `Event::Scroll` was the one variant of the
  protocol with no doc comment saying what its number means. The viewer
  kept a literal `15.0`, `input_win` kept its own, and the KVM invented a
  third. Fixed the way `point_from_pixel` was fixed:
  `input::SCROLL_PX_PER_LINE` and `input::wheel_notches` now live in the
  protocol module, platform-neutral, with tests.
- **The conversion sat where no test runs.** See
  [[check-the-other-cfg]]: `input_win.rs` is `#![cfg(windows)]`, so a test
  written in that file never runs in `scripts/check.sh` -- the cross
  target compiles it and nothing executes it.

## Rule

New host, or a new screen on an existing host: **measure both spaces
before trusting they are one**. The test is `--abs` in `input-send`, and
the case that reveals it is the **middle** of the frame — not the
corner, which the clamp disguises, and not relative movement, which
doesn't even go through the conversion.

```bash
oxdesk-input-send <host>:<port+1> --abs 1680 1050   # middle of the frame
```

And for any number the protocol carries: **the unit is named once, in
`stream/src/input.rs`, next to the event that carries it.** A literal
repeated at both ends is not a convention -- it is two conventions that
happen to agree until one of them is edited.

## See also

- [[../skills/verify-the-mac]] — the recipe with the four points
- [[cursor-without-mouse]] — the other cursor problem, unrelated to this one
- [[verify-what-arrived]] — why `--rel` saying "exactly +200" proved
  nothing here
