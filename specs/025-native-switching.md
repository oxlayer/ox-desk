# 025 — Switching that feels native

> Written for: whoever builds the next version of the LAN KVM, and
> whoever reviews it.

Spec 024 made Windows a KVM source and brought a switch down to ~35 ms.
What is left between "it works" and "it feels like one computer" is
structural, and four pieces fix it together: a link that stays open, a
capture that does not touch the local cursor, edges that know the
monitors, and a way back decided by the machine whose cursor it is. This
spec designs them, plus the safety nets they need and the proof each one
owes.

The ideas were found by reading another open-source KVM,
[`EduardoMG12/mickey`](https://github.com/EduardoMG12/mickey) (GPL-3.0).
Its code is not ours to take -- OxDesk is AGPL-3.0 with an inbound
licence policy ([`021`](021-licensing-and-contributions.md)) -- so what is
written here is a design in our own terms, measured on our own machines.
Where a mechanism comes from there, it says so.

## What is wrong today

| Today | The cost |
|---|---|
| A new `oxdesk-kvm` process and a new QUIC connection per switch | ~35 ms warm, more cold; a switch can fail on a name that does not resolve at that moment |
| Mouse motion read from the low-level hook, against a cursor parked at the centre of the primary monitor | Windows' pointer acceleration is applied, then the target's on top; the local cursor visibly jumps to the middle of the screen |
| The Windows edge polls `GetCursorPos` every 30 ms against one rectangle around all monitors | Gaps between monitors of different sizes count as edges, and a cursor sliding along an edge can fire |
| The way back is decided by the source from a cursor it imagines (spec 024) | It drifts from the real one when the target's own pointer moves; the 40 px push is a guess |
| Ubuntu injects through Mutter's private RemoteDesktop API, which needs a screen-capture session | The host must capture video to accept a mouse; a renamed method failed silently for weeks |

## 1. One link per peer, kept open

**Design.** The daemon opens one input link to each peer it may send to,
when it starts and whenever the peer comes back, and keeps it. A switch is
a message on that link, not a process: `Enter { x, y }` to start, events
while the input is away, `Leave` from the other side to end it.

- The grab stays a separate process (`oxdesk-kvm`), because a process is
  what guarantees the keyboard is released when something crashes
  (spec 005). What changes is that it is started once and kept warm,
  holding the link and **not** the grab, and told to grab or release over
  a pipe. A daemon restart ends it, and with it any grab.
- Liveness both ways: a heartbeat every second; the source gives the
  input back after 4 s of silence, the target releases every held key
  after 10 s (mickey uses the same pair of numbers, `source/mod.rs:59`,
  `net/transport.rs:59`).
- Bulk data (clipboard, files) never shares the input stream: a key press
  must not wait behind an image. QUIC streams already give us that; keep
  input on its own stream.

**Proof.** Switch time measured from the edge touch to the first event
applied on the target, not from the daemon's log: a synthetic move on the
source and a timestamp on the target, both clocks read on one machine
through the link's round trip. Target: under 10 ms on the LAN, every time,
including the first switch after an hour idle.

## 2. Capture that leaves the local cursor alone (Windows)

**Design.**

- **Motion from Raw Input**, not from the hook: a message-only window
  registered with `RIDEV_INPUTSINK` receives the device's own counts,
  before Windows' acceleration. The target applies its own, once.
- **The hook still swallows** every event while the input is away (that
  part stays as it is in `grab_win.rs`), and the cursor **stays where it
  was**: no `SetCursorPos` to the centre.
- **Hiding the cursor**: an invisible, click-through, topmost window over
  the virtual desktop with a null cursor, shown only while the input is
  away. mickey does this and pays for it by taking the foreground, which
  closes open menus (`platform/hooks.rs:402-434`). We try first without
  taking focus (`WS_EX_NOACTIVATE`), measure whether the cursor hides, and
  record the result either way.
- **Keep the rescue in the hook thread.** mickey's escape runs in its main
  loop (`hooks.rs:584-600`): a hung loop keeps swallowing input. Ours is
  decided where the events arrive, and stays that way.

**Measured first, not assumed (2026-11-16): swallowing an event in the
low-level hook does not hide it from Raw Input.** `oxdesk-probe-rawinput`
injects moves tagged with a marker, swallows only those in its hook, and
counts the tagged ones a `RIDEV_INPUTSINK` window still receives. On the
reference Windows machine, two runs:

```
hook OFF: injected 50, swallowed by the hook 0, seen by Raw Input 50
hook ON : injected 50, swallowed by the hook 50, seen by Raw Input 50
```

The control (hook off) seeing all 50 is what makes the second line mean
something. Still to confirm with a real mouse, which `SendInput` stands in
for here: the same count while a person moves it.

**What was built first, and why not Raw Input yet (v0.3.7).** The visible
problem was the cursor, not the motion: parked at the screen centre, it sat
in the middle of the person's Windows screen for as long as the input was
away. And Raw Input changes the feel on a GNOME target: there the source
places the cursor with absolute positions (spec 024), so today the only
acceleration is Windows' own, applied once, which is the feel that was
liked. Raw Input's counts carry none; they need a curve of our own first.

So the cover came first: a window over the desktop, alpha 1, non-activating,
answering `WM_SETCURSOR` with no cursor (`grab_win::cover_desktop`).
Measured on the reference Windows machine with `GetCursorInfo` and
`GetForegroundWindow`, sampled every 200-250 ms through a grab:

| When | Cursor | Foreground window |
|---|---|---|
| before | showing, a cursor set | unchanged |
| during | not showing, no cursor | unchanged |
| after | showing, a cursor set | unchanged |

The same foreground window throughout is the part mickey pays for: its
cover takes the foreground, which closes menus. `OXDESK_KVM_SHOW_CURSOR=1`
turns the cover off, for diagnosing.

## 3. Edges per monitor, driven by motion

**Design.** The edge is "the next pixel the cursor would reach is on no
monitor", checked per axis, against the list of monitors, re-read when
the display configuration changes. A push past a monitor's edge into a
gap that another monitor covers is not an edge; a slide along an edge is
not a push. The check runs on motion (Raw Input while local), throttled to
one evaluation per ~8 ms, instead of polling every 30 ms.

The entry point on the other machine is the same fraction along the edge,
mapped onto the monitor of the target that faces this machine.

**Proof.** A pure function over a monitor list and a motion, with tests
for: two monitors of different heights side by side, a gap, a slide along
an edge, a push at a corner.

**Built on Windows (v0.3.6).** The reference Windows machine has exactly
the case: a 1920x1080 monitor left of the 2560x1440 primary. With the box,
the bottom 360 px of the primary's left side were an edge nobody could
use: the cursor stops at x = 0 there, and the box's edge is x = -1920.

- `edge_state::monitor_edge_distance` decides per monitor; its tests use
  that exact layout. The watcher re-reads the monitors every second.
- `input::return_point` brings the cursor back onto a real monitor at that
  height: high on the left lands on the left monitor, low on the left on
  the primary, where the box would have put it off every screen.

Measured on the machine: in the old dead zone, at (100, 1200), the
approach woke the other side's host (`host is up ahead of the edge`); the
control, at (100, 500) beside the left monitor, woke nothing. Still
polled at 30 ms, not driven by motion: that comes with Raw Input.

## 4. The other machine decides the way back

**Design.** The target watches its own cursor against its own monitors,
with the same function as section 3, and sends `Leave { edge, along }`
once per push, never while a mouse button is held (a drag across the edge
is not a request to go home). The source ends the grab and puts its cursor
at the matching edge, at the same fraction along it.

- Stale answers are discarded: a `Leave` that arrives after a new `Enter`,
  or after the focus moved, is ignored. That is what stops two machines
  from bouncing the input between them.
- This replaces the virtual cursor and the 40 px push of spec 024, which
  stays as the fallback for a host that does not send `Leave`.

**Proof.** The target's decision is the pure function of section 3; the
round trip is proven on the reference deployment, both directions, and in
the VMs of section 7.

## 5. Ubuntu as target without a screen capture: the RemoteDesktop portal

**Design.** On GNOME 46 and later (the reference Ubuntu machine runs
GNOME 50.1), inject through the RemoteDesktop portal with libei
(`ConnectToEIS`) instead of Mutter's private API: an absolute pointer
whose regions are the monitors, no ScreenCast session, and a restore
token so the permission dialog appears once. mickey measured the next
session starting in 36 ms, and 50 ms after a reboot, with no dialog
(PRD 0004). Keys can go through the same libei connection.

Mutter's private API stays as the fallback for older GNOME, now with the
first call of each method checked (spec 024).

## 6. Safety nets

- **A way home no setting can remove**, independent of the chosen pair
  (for example, holding Right Shift for 3 s), read where the rescue is.
- **A key held during the switch comes up where it went down.** mickey
  makes the grab wait until every key is up; we chose not to delay the
  switch. **Built (v0.3.9)**: the grab reads which keys and buttons are
  down when it starts (`GetAsyncKeyState`), and their release goes to this
  machine while their auto-repeat goes nowhere (`input::LocalHeld`).
  Before, crossing while holding Ctrl left Ctrl down on Windows: its
  release was swallowed and forwarded, and the next click at home was a
  Ctrl+click. Confirmed by hand on the reference deployment, with the
  session's own count: `1 release(s) of keys or buttons held from before
  the grab went to this machine`, 0 keys forwarded, and Windows reporting
  nothing held afterwards.
- **No edge switch during a drag** (also v0.3.9): while a mouse button is
  held, the edge does not fire, and an edge reached during the drag fires
  only after the cursor leaves and comes back. Moving a window to the
  screen edge is not a request to change machine. Confirmed by hand.
- **Held keys tracked on both ends**: the target releases what it pressed
  when the link ends, whatever the reason. **Built (v0.3.8)**:
  `input::Held`, in all three hosts. Proven on the Ubuntu host with
  `oxdesk-input-send --leave-held 42` (Shift down, then disconnect, as a
  crashed source would): the old host released nothing, so Shift stayed
  down until released by hand; the new one logged `released 1
  key(s)/button(s) the client left held`.
- **Gamepad virtual keys are not "a key held"**: a half-pressed trigger
  must not block a switch (mickey found this the hard way,
  `hooks.rs:565-582`).

## 7. Proven in virtual machines, end to end

mickey boots throwaway VMs, installs the build, and drives them with
synthetic clicks and keys (`tools/distro-test/`). For OxDesk:

- **Two VMs, Ubuntu and Windows, on a private network,** built from a
  script, with the release under test installed by the same installer a
  user runs.
- **A driver on the host** that, inside the source VM, moves the mouse to
  the edge, types a known string, and pushes back; and inside the target
  VM reads what arrived (the cursor position from the compositor, the
  typed text from a text field). Pass means the string arrived whole and
  the cursor came back.
- **Run on every release candidate**, not on every PR: it is slow, and the
  PR gate stays fast. A failure blocks the release tag.

This is its own spec when it is built; it is listed here because every
section above names a proof the VMs can run without a person at the
keyboard.

## Order of work

Each step ships on its own, behind the previous one:

1. The persistent link (1), keeping today's grab and the 40 px fallback.
2. Raw Input and the cursor left in place (2), after the hook/Raw Input
   measurement.
3. Per-monitor edges (3) on Windows, then the target-decided way back (4).
4. The safety nets (6), each with its test.
5. The portal route on GNOME (5).
6. The VMs (7), in parallel from step 1, so later steps are proven there.

## The case against

**"QUIC per switch is fine at 35 ms."** It is, until it is not: a name
that does not resolve, a host that is waking, a process that takes a
moment to start. Each of those happened on 2026-11-15. An open link makes
every one of them a non-event, and it is what the target-decided way back
needs anyway.

**"An invisible window over everything is a hack."** It is the hack every
software KVM on Windows uses, because Windows has no "hide the cursor
globally" call. The design measures the focus-stealing version against a
non-activating one before choosing.

**"Copy mickey."** No: licence, and because measuring on our machines is
what made every spec here trustworthy. Their numbers are leads, ours are
the results.

## See also

- [`005`](005-input-switch.md) — the LAN KVM and why the way back must not
  depend on the network
- [`023`](023-shortcut.md) — the shortcut's pair
- [`024`](024-windows-kvm-source.md) — Windows as the source, the first
  way back by the edge, and the switching speed
