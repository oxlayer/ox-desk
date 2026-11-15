# Windows without a mouse has no cursor — and that breaks the KVM in two ways

A Windows machine with **no** pointing device connected hides the cursor
completely. Measured:

```powershell
Get-PnpDevice -Class Mouse | Where-Object Status -eq OK   # NONE
# and, in the interactive session:
GetCursorInfo -> ok=True flags=0 hCursor=0 pos=1356,728
```

`flags=0` is hidden, `hCursor=0` is "no cursor exists". **The position
is still correct** — that's why hover responds and nothing shows up,
which is exactly how the symptom is reported: hover effects follow the
mouse, but no cursor is visible.

It is the normal state of a machine driven only by KVM: the dongle is at
the other end.

## Consequence 1: DXGI has nothing to deliver

`AcquireNextFrame` only reports a pointer when one is being drawn.
Without a cursor, `LastMouseUpdateTime`, `PointerShapeBufferSize` and
`Visible` come back zeroed forever. Asking the cursor shape from
something that has no cursor is circular — instrumenting DXGI doesn't
help.

Solved by drawing our own arrow (`cursor::builtin_arrow`), with the
position coming from `GetCursorPos`, which works whether the cursor is
visible or not.

## Consequence 2: drawing in the stream doesn't help whoever looks at the monitor

And here lies the verification trap. The composited arrow exists
**inside the video**. Whoever is watching through the viewer sees it;
whoever is looking at the machine's physical monitor **sees nothing**,
because there Windows still has no cursor.

The fix was first confirmed by decoding the h264 and looking at the
frame — the arrow was there, at the right pixel — and reported as
solved. The report had come from someone looking at the physical
monitor. The right thing had been verified in the wrong place.

**Before declaring a visual problem solved, check WHERE the person is
looking.** The host log tells you: a session with `input connected` and
no video `connected:` at all means KVM without a viewer — that is, the
person is in front of the physical monitor. (Builds from before the
English rename log these as `input conectado` and `conectado:`.)

## What Parsec does (and what doesn't work)

Parsec documents exactly this problem. Their solution is their own
**virtual USB mouse driver** — a mouse that doesn't move, it only exists
so Windows goes back to drawing the pointer. It's not generic: it
depends on their driver.

Revealing: their documentation says Parsec *"reproduces the hidden
cursor faithfully"*, i.e. the client sees no pointer at all. Our
synthetic arrow does more than that in the viewer.

**Mouse Keys doesn't solve it** — Parsec suggests it, and we measured
that it doesn't work on the reference Windows machine (Windows 10):

```
Flags no registro: 63      (MouseKeys LIGADO)
GetCursorInfo:     flags=0 hCursor=0
```

(The first line reads "Flags in the registry: 63 (MouseKeys ON)".)

There is also no API call that solves it: the functions that should show
the cursor (`ShowCursor`) have no effect without a mouse present. That's
why everybody ends up in hardware or a driver.

## The real fix: give Windows a mouse

Any HID pointing device makes Windows go back to drawing the cursor —
including a **Unifying receiver with nothing paired**, which enumerates
mouse and keyboard interfaces on its own.

That is better than our arrow in two ways: it shows up on the physical
monitor, and DXGI goes back to sending the real shapes (hand, text bar,
resize), which we now draw instead of ours.

The alternative in code would be an overlay window, transparent to
clicks (`WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST |
WS_EX_NOACTIVATE`) following `GetCursorPos`. It works on the physical
monitor and is captured by DXGI along with it, but it is extra Win32 to
maintain and doesn't get along with exclusive fullscreen games — which
is precisely the KVM use case on the reference Windows machine.

## See also

- [[windows-locked-screen]] — another family of symptoms of the same host
- [[../../specs/005-input-switch.md]] — the LAN KVM

## Outcome (2026-11-04)

A spare Unifying receiver was plugged into the Windows machine. The
result, measured:

```
Get-PnpDevice -Class Mouse  ->  HID-compliant mouse
host log                    ->  forma de ponteiro: Cor 32x32 pitch=128 (4096 bytes)
                                cursor desenhado em 1356,728 (32x32)
```

(The host log lines read "pointer shape: Color 32x32 pitch=128 (4096
bytes)" and "cursor drawn at 1356,728 (32x32)"; current builds print them
in English.)

A Unifying receiver **with nothing paired** is enough: it enumerates
mouse and keyboard interfaces by itself. Windows went back to drawing
the cursor on the physical monitor, and DXGI went back to sending the
real shape — colored with alpha, 32x32 — which we now composite instead
of our arrow.

The built-in arrow stays in the code and is still right: it covers
whoever watches a machine that has no mouse at all. What changed is that
it is no longer the normal path for the reference Windows machine.

With a system cursor present, DXGI's `Visible` started to mean
something, so we respect it again — a fullscreen game hides the pointer
on purpose, and drawing one on top would be intruding. Without a mouse,
`Visible` is always false and obeying it would erase precisely the
fallback.
