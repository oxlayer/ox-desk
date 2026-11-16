# 024 — Windows as the KVM's source

> Written for: whoever touches how input leaves a Windows machine, and
> whoever reviews that change.

With the Logitech receiver plugged into Windows, the keyboard and mouse
can now be sent to another machine the same three ways Linux always could:
the shortcut, the screen edge, and "Send to" in the panel or the tray.
Windows grabs them with low-level hooks and forwards them over the LAN
KVM's input stream; the other machine's screen host injects them. This
spec records what was broken, what the design is, and how it was proven.

## What was broken (2026-11-15)

The single receiver moved from the Ubuntu machine to the Windows machine.
From then on nothing could move the input off Windows:

| Path | What happened |
|---|---|
| Both Ctrl | The shortcut reader existed on Linux only. Nothing listened. |
| "Send to ubuntu-host" | `kvm::send` on Windows asked the Ubuntu machine to release a grab that did not exist (`kvm.rs`: only Linux could be the source). |
| The edge | It used the HID++ channel switch whatever `[input] mode` said. With the only receiver on Windows, channel 1 has nobody listening: the switch would have stranded the keyboard and mouse until someone pressed the device's Easy-Switch button. The pre-flight's dead-man's-switch check refused it -- by luck of a cold cache, and that check passes once the peer's agent answers. |

The edge was turned off on the Windows machine until the LAN path below
existed.

## The design

- **`stream/src/grab_win.rs`**: `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks
  on a thread with its own message loop. Each real event is translated to
  the protocol's `Event` and swallowed; injected events pass through
  untouched (they are our own host typing what another machine forwards).
- **The rescue is decided in the hook thread.** The forwarding loop awaits
  QUIC writes; a dead peer could hold it, and a rescue decided there would
  die with the network while every key stays swallowed. The hook sees the
  pair and unhooks on the spot.
- **Mouse movement is read against a parked cursor**: park at the primary
  monitor's centre, report `pt - park`, swallow, so the cursor never moves.
  On release the cursor goes back where it was, moved off any screen edge,
  so the edge does not read the restored cursor as a new arrival.
- **Keys** come from the hook's scancode through the inverse of the table
  the Windows host already uses to type (`input::evdev_from_windows_key`),
  so the two directions cannot drift. AltGr's fake left Ctrl (`0x21D`) is
  dropped: taken as a real one it would be half of the default rescue.
- **`oxdesk-kvm` on Windows** connects first and grabs after, like Linux.
  The daemon ends a grab by closing the child's stdin, which releases the
  keys still held on the other side; killing would skip that. A daemon
  that dies closes the pipe too, so a grab never outlives its owner.
- **The daemon is the source when the devices are here**
  (`hidpp::devices_here()`); otherwise "Send to" still asks the machine
  that has them.
- **The edge follows `[input] mode`**: LAN (the default) starts the KVM;
  only `hidpp` switches the radio channel, and only then does the
  dead-man's switch run.
- **The shortcut on Windows** is a watching hook in `winput::keys` that
  never swallows; `oxdesk/src/hotkey_win.rs` feeds the decision shared with
  Linux (`hotkey::fire`). While the KVM holds the grab its hook runs first
  and does not pass events on, so the two never answer the same press.

## Found on the way, and fixed

- **The daemon declared success before the KVM did anything.** It waited a
  fixed 500 ms for the child to die. On Windows the name lookup alone took
  longer, the child died after the check, and the panel said "input going
  to ubuntu-host" about a keyboard that never left. The daemon now waits
  for the child's own `grabbed` line (up to 8 s), and kills it at the
  deadline rather than let a late grab take a keyboard it said it would not.
- **mDNS flaps on Windows.** The Ubuntu machine's `.local` name stopped
  resolving for ~20 s every few minutes, and the child died with
  `os error 11001`. The daemon now resolves on Windows and passes the
  address, with the last known one as fallback (`probe::resolve_with_memory`):
  in memory only, replaced at the next answer, and checked by the pin.
- **The KVM connected unpinned.** Every keystroke crossed a connection
  whose host identity was not checked (spec 017). It now gets `OXDESK_PIN`
  like the viewer.
- **The Ubuntu host recorded the wrong screen.** It defaulted to the
  connector `DP-1`; the monitor had moved to `DP-3`. The host recorded an
  invisible virtual screen and the input went there: 635 moves forwarded,
  "500 input events applied", and a cursor that never moved. It now asks
  Mutter for the primary monitor (`host_linux::primary_connector`).
- **`oxdesk-kvm.exe` was not in the Windows package**, the wizard's list,
  or the deploy. Now it is in all three.

## How it was proven

On the reference Windows machine, in order:

1. `--self-test 40` (connects, does not grab): the Ubuntu host logged the
   moves.
2. A grab nobody touched forwarded 0 events, which cannot tell "nobody
   touched" from "the hook saw nothing". So the KVM now prints what it
   forwarded by kind, and what the hooks passed through; synthetic moves
   injected during a grab were passed through, 10 of 10: the hooks run.
3. Through the daemon (`POST /api/focus`, then `/api/kvm/release`): resolved
   by address, pinned, grabbed, and forwarded what the person was doing
   (4 keys, 2 buttons, 635 moves, 1 scroll), then released cleanly.
4. After the monitor fix: the host recorded the real 2560x1440 screen and
   forwarded 405 moves and 46 scrolls.

## The case against

**"A Raw Input reader would give unaccelerated movement."** It would, and
the target applies its own acceleration on top of Windows' today. But
whether a low-level hook that swallows also starves Raw Input is not
something this repo has measured, and a grab that reads nothing is worse
than one that reads accelerated movement. Measure first.

**"Hooks can be removed by Windows."** A callback slower than
`LowLevelHooksTimeout` is dropped without notice. The callbacks only
translate and queue; and the failure, if it came, is the safe one: the
input stays on this machine.

**"Ctrl+Alt+Del is not grabbed."** It cannot be: it is the secure
attention sequence. It acts on this machine, as it should.

## Not yet

- Coming back by the other machine's screen edge. Today the way back from
  a Windows grab is the shortcut, the panel, or the tray.
- Raw Input for movement (see above).

## See also

- [`005`](005-input-switch.md) — the LAN KVM, connect-then-grab, the rescue
- [`017`](017-product.md) — host identity and the pin
- [`023`](023-shortcut.md) — the shortcut's pair and the switching settings
- [[../.claude/rules/graphical-session]] — why the hooks only work inside the session
