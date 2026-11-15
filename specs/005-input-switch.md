# 005 — Input routing (the core)

This is the piece that does not exist in open source. See
[`000`](000-overview.md#the-gap-that-justifies-the-project).

## The idea

When the cursor touches the edge (or you press a shortcut), there are **two
ways** to hand control to the other machine:

| Route | How | Latency | When |
|---|---|---|---|
| **physical** | tells the mouse/keyboard to jump Easy-Switch channel to the other machine's receiver (HID++ 0x1814) | native — it is the device's own radio | the other machine has a receiver with the device paired |
| **LAN** | Deskflow transmits events over the network | ~1 ms + network, and depends on portal/libei | the other machine has no receiver, or the channel is not paired |

Flow does the first. Deskflow does the second. **Neither chooses between
the two.** Choosing is the product.

The physical route is better when available: native latency, it works with
the network down, and it does not depend on input injection through the
portal — which **works around the Deskflow server-mode segfault on
Linux**. That one was measured and is worth keeping: `deskflow-core server`
1.27.0 segfaulted on startup (exit 139), reproduced twice, with an
isolated config and with the real one. It crashed **before reaching the
portal**, so it was not the GNOME `InputCapture` gap the Deskflow
documentation describes — that was ruled out separately, since
`gnome.portal` does declare `org.freedesktop.impl.portal.InputCapture` and
accepts `CreateSession` v1. It was a bug in that build's server mode, and
the practical consequence at the time was that the Windows machine stayed
the server.
Today both machines have a Unifying receiver, so the physical route is
possible in both directions.

## Edge detection: mutter directly, not the portal

Correction of an earlier claim. This spec said "everything through the
portal, no hacks", based on having seen
`org.freedesktop.portal.InputCapture` **accept** `CreateSession`.
Accepting is not responding.

### The portal accepts and never responds

```
CreateSession  -> devolve request path
Response       -> NUNCA chega (rodada limpa, 75 s, confirmada)
backend        -> 43 requests pendentes acumulados, nenhum resolvido
no fechamento  -> "Object does not exist at .../session/icsess0"
```

(In English: `CreateSession` returns a request path; `Response` NEVER arrives
— clean run, 75 s, confirmed; backend: 43 pending requests piled up, none
resolved; on close: the "Object does not exist" error.)

It is not a missing implementation: `xdg-desktop-portal-gnome` has
`InputCaptureDialog` with a permission switch, `AddBarrier`,
`ClearBarriers`, `GetZones`, `ConnectToEIS`. What it cannot do is
**show the consent dialog** —
`Failed to associate portal window with parent window`. A CLI without a
window has nothing to pass as `parent_window`, and the request hangs
forever. The permission store confirms it: there is a table for
`remote-desktop` and `screencast`, **none** for `input-capture`.

### `org.gnome.Mutter.InputCapture` works, and it is synchronous

The portal backend is just an intermediary for a mutter service that
is on the bus, served by gnome-shell, and answers direct calls —
no Request/Response, no dialog:

```
org.gnome.Mutter.InputCapture
  CreateSession(u capabilities) -> o session_path
  SupportedCapabilities = 15

org.gnome.Mutter.InputCapture.Session
  GetZones()                     -> (u serial, a(uuii) zones)
  AddBarrier(u serial, (iiii))   -> u id
  ClearBarriers() / Enable() / Disable() / Close()
  ConnectToEIS()                 -> h fd
  Release(a{sv} options)
  signals: Activated(u barrier_id, u activation_id, (dd) cursor)
           Deactivated(u activation_id), ZonesChanged, Disabled, Closed
```

### Validated with hardware (2026-11-03)

```
CreateSession(2)   -> /org/gnome/Mutter/InputCapture/Session/u5
GetZones()         -> serial=0, zones=[(2560, 1440, 0, 0)]
AddBarrier(x=2560) -> id=1
ConnectToEIS()     -> fd=8
Enable()           -> ok

*** ACTIVATED *** barreira=1 activation=1 cursor=(2560.0, 813.23)
[Release] ponteiro devolvido
[Deactivated] (1,)
*** ACTIVATED *** barreira=1 activation=2 cursor=(2560.0, 813.23)
[Release] ponteiro devolvido
```

(`barreira` = barrier; `ponteiro devolvido` = pointer given back.)

**It is possible to use only the detection** and give the pointer back right away,
without capturing or forwarding input via libei. That is the trigger for the
physical route, coming for free.

### Three things that took several attempts

**The barrier goes on the zone's BOUNDARY, not on its last column.**
With `x = x + w - 1` (2559) mutter refuses:
`AccessDenied: Line overlaps with monitor region`. It has to be
`x = x + w` (2560).

**`ConnectToEIS` before `Enable`.** It was not confirmed to be
mandatory — it went into the test to eliminate the hypothesis, and the event
arrived with it present. It stays as the known-good order until someone measures
without it.

**Injected vs. real movement.** The first test timed out with the
devices on the Windows channel: the pointer on the Ubuntu machine was
injected by Deskflow via libei, and injected movement may not cross a compositor
barrier. The event only came with the device **on the local channel**. That
is not a limitation — it is exactly mode B, where the device is local by
definition.

## Prerequisite: hidraw access

HID++ needs to open `/dev/hidraw*`. The Solaar rule
(`/usr/lib/udev/rules.d/60-solaar.rules`) grants it via `TAG+="uaccess"`,
which becomes an ACL for the logged-in user.

**Known gotcha:** the `hidraw` nodes were created at 11:37 and Solaar was
installed at 13:22 — the rule did not exist when the nodes were created, no ACL
was applied, and `solaar config` died with `No supported device found`,
with `getfacl /dev/hidraw0` showing only `user::rw-` (root).

```bash
sudo udevadm trigger --subsystem-match=hidraw --action=add   # or replug the receiver
getfacl /dev/hidraw0 | grep "^user:$USER:"                 # confirm the ACL
```

`oxdesk` should **check this at startup** and report it in the API/GUI when it is
missing, instead of failing obscurely at switch time.

Mapping on the Ubuntu machine of the reference deployment:

| hidraw | device |
|---|---|
| `hidraw0` | `046D:C52B` — Unifying receiver |
| `hidraw2` | `046D:4090` — MX Keys |
| `hidraw3` | `046D:408A` — MX Anywhere 3 |

## Discovered constraint: only the holder of the device can push it

Tested: with the mouse on the Windows channel, `solaar config 1` fails with
`no online device found matching '1'`, and `solaar show` lists both
devices as `offline`.

The reason is physical: the HID++ command leaves the receiver by radio to the
device. If it is listening to **another** receiver, there is no way to
reach it. That is:

> The channel switch can only be issued by the machine that **has the
> device at that moment**.

This makes the architecture necessarily **symmetric**: the control
plane does not execute the switch, it **asks the current holder** to execute it.
Whoever is without the device has no way to bring it back via the physical route.

### The Windows side: solved

Solaar is Linux-only, so Windows needed its own HID++.
Implemented in [`agent/`](../agent/): Rust, cross-compiled from Ubuntu
(`x86_64-pc-windows-gnu` + mingw from apt), **352 KB**, no runtime.

It is a **CLI, not a server**, invoked over SSH. It reuses the key
that already exists: zero new ports, zero new auth, zero network surface.

```
oxdesk-agent devices                      # slots, number of channels, current channel
oxdesk-agent host-info <slot>
oxdesk-agent set-host <slot> <n> --confirm
oxdesk-agent input-age                    # ms since the last input
oxdesk-agent guard <slot> <n> <seconds>   # dead-man's switch
oxdesk-agent raw <slot> <feature> <func>  # protocol probing
```

HID++ via `hidapi` (integrate rather than reimplement: HID enumeration and I/O
on Windows are ~300 lines of unsafe FFI already solved there).

#### The HID collections gotcha on Windows

First attempt: write failed with an empty `hidapi error:`. Cause:
on Windows the Logitech receiver exposes **one HID collection per report
type** — usage 1 = short (`0x10`, 7 bytes), usage 2 = long (`0x11`,
20 bytes), usage 4 = very long. The code opened the first one in the list
(usage 2) and wrote a short report to it. Wrong size, write refused.

Solution: **always use the long report on the usage 2 collection**, even for
functions that would fit in the short one. That way writes and reads stay in the same
collection — the alternative would be to open two and correlate between them.

On Linux this problem does not exist: a single `hidraw` carries all
report types.

### Read from the hardware (2026-11-03)

```
slot 1 (MX Anywhere 3):     3 canais, atual = 2
slot 2 (MX Keys Keyboard):  3 canais, atual = 2
0x1815 func 0:  13 04 03 01   -> numHosts=3, currentHost=1 (0-based)
0x1815 func 1:  os três canais com status=1 (pareado)
```

(Program output: each device has 3 channels, current = 2; func 1: all three
channels with status=1, paired.)

So **the Windows machine is channel 2**, confirmed, and the devices
know three hosts — all paired.

The Ubuntu machine's channel **has not been read yet**, and it does not go into the config
as a guess: sending the device to a wrong channel leaves it
unreachable from both machines. The risk-free discovery is a keypress:
press Easy-Switch until the device shows up on Ubuntu, and then
`solaar config <slot> change-host` reports the current channel.

> Reading the channel names via `0x1815` func 2 was attempted, and the layout
> did not yield anything (`02 80 00…`, no ASCII). The attempt was dropped: the
> likely write function sits next to the read one in the same feature, and
> probing function indexes blindly could **rename** a host. Not worth the risk
> for information that a keypress resolves.

### There is no rollback on this side

Correction of an earlier design error: the first version of this spec said
"if the target does not confirm, go back to the previous channel". **That is
impossible.**

`ChangeHost` travels by radio to the device. Once it switches
channel, this receiver **can no longer reach it** — proven by
`solaar config 1` failing with `no online device found` precisely
because the device is on the Windows channel. So the switch is
**irreversible on the source side**, and there is no return point: the
instant after the command, we are blind and without input.

Two design consequences:

**1. Pre-flight replaces rollback.** Every check happens
*before* the irreversible act, because afterwards there is nothing to do. That is why
`POST /api/focus` is **dry-run by default** and only executes with
`"confirm": true` — the general rule for destructive triggers: the action
is irreversible, so preview is mandatory.

**2. The dead-man's switch lives on the TARGET.** Whoever can give the
device back is whoever comes to hold it. This inverts the responsibility: the
source does not try to save itself, the destination rescues it.

### Implemented inside `watch-edge` — and why there

First idea: `oxdesk` would ask the target's agent, over SSH, to run
`guard`. **It does not work.** `GetLastInputInfo` is per session, and a command over
SSH lands in a session that has never seen input — measured: it reports the machine's
uptime (704577031 ms ≈ 8 days).

So the guard went inside `watch-edge`, which is already resident in the
user's session and already polls:

1. detects that the devices **arrived** (transition from absent to
   present, checked at 1 Hz);
2. records the idle at that instant and opens a `OXDESK_GUARD_SECS` window
   (25 s by default);
3. if the idle drops below the time since arrival, there was activity — and
   it disarms;
4. if the window closes without input, it issues `ChangeHost` back.

Validated on the first run:

```
watch-edge START borda='left' area=(0,0 4480x1440) slots=[1, 2] -> host 1
dispositivos CHEGARAM -- vigiando input por 25s (idle na chegada: 0ms)
input detectado -- dead-man's switch desarmado
```

(Log: devices ARRIVED — watching input for 25 s, idle on arrival 0 ms; input
detected — dead-man's switch disarmed.)

> The presence check is HID++ and the loop spins every 30 ms so the cursor
> responds quickly. Without a limit, that would be 33 pings per second hammering the
> radio — it runs at 1 Hz, and the guard window is tens of seconds.

Another data point from the same log: the virtual area went from `2048x1152` to
`4480x1440` between runs — the machine gained a second monitor. That is the
practical argument for using `SM_XVIRTUALSCREEN` and family instead of
`SM_CXSCREEN`, which would only see the primary.

### Recovery without input

If both sides fail, the user is without keyboard and mouse. The
remaining paths, in order:

1. the Easy-Switch button on the device itself (physical, always works);
2. any USB keyboard;
3. SSH from another machine to the Ubuntu machine.

The GUI has to show item 1 **before** any switch, not after.

## Config

```toml
[input]
prefer = "physical"        # "physical" | "lan" | "auto"
confirm_timeout_ms = 3000

[input.border]
enabled = true
edge = "left"              # edge of this machine that leads to Windows
hold_ms = 250              # avoids an accidental switch when grazing past

[input.shortcut]
enabled = true
accel = "<Super><Alt>Right"

[[devices]]
name = "MX Anywhere 3"
hidraw_id = "046D:408A"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }   # Easy-Switch channel

[[devices]]
name = "MX Keys"
hidraw_id = "046D:4090"
hosts = { "ubuntu-host" = 1, "windows-host" = 2 }
```

The `hosts` map is what Solaar does not have and Flow keeps in the cloud.
Here it lives in a TOML of your own.

## Validated end to end (2026-11-03)

Complete cycle, in both directions, with real hardware:

```
# Windows -> Ubuntu, through the agent via SSH
oxdesk-agent set-host 1 1            -> recusa: falta --confirm
oxdesk-agent set-host 1 1 --confirm  -> {"ok":true,"slot":1,"host":1}
oxdesk-agent set-host 2 1 --confirm  -> {"ok":true,"slot":2,"host":1}
   Ubuntu:  slot 1 e 2 online=True
   Windows: nenhum slot online

# Ubuntu -> Windows, through oxdesk
POST /api/focus {"host":"windows-host","confirm":true}
   executado: true
   MX Anywhere 3:    slot 1 -> canal 2
   MX Keys Keyboard: slot 2 -> canal 2
   Ubuntu:  online=False
   Windows: slot 1 e 2 ONLINE canal=2
```

(Output: the first call is refused for missing `--confirm`; afterwards Ubuntu
sees slots 1 and 2 online and Windows none; in the other direction both slots
go to channel 2 and are ONLINE on Windows.)

**This is the Logitech Flow behavior, without cloud and without an account**:
the piece no open project had.

### `input-age` over SSH is useless — proven

```
$ ssh windows-host oxdesk-agent input-age
{"idle_ms":704577031,"ok":true}        # ~8.15 days
```

8.15 days is the machine's **uptime**. The SSH session never saw input,
so `GetLastInputInfo` returns the time since boot. This confirms by
measurement the session isolation described in
[`002`](002-ssh.md#structural-limit-session-isolation): `guard`
**cannot** be triggered over SSH, it has to be resident in the interactive
session (shortcut in `shell:startup`).

## Two working modes, now that the physical route exists

| | Devices on | How it switches | Price |
|---|---|---|---|
| **A** (current) | Windows | cursor at the edge, via Deskflow | Ubuntu input depends on Windows being on |
| **B** (new) | Ubuntu | `POST /api/focus` | native latency, without depending on Windows — but the edge trigger is missing |

Mode B is the best for someone who has Ubuntu as their main machine, and
depends only on edge detection through the `InputCapture` portal to become
indistinguishable from Flow.

## Implemented in Rust and measured (2026-11-03)

[`oxdesk/src/border.rs`](../oxdesk/src/border.rs), via the `dbus` crate
(libdbus), **not** zbus: measured, zbus took the tree from 5 to **189
crates**, dragging in an async runtime to make six D-Bus calls.
With `dbus 0.9.12` it stayed at **40**, at the cost of `libdbus-1-dev` from apt.
`zbus 6.0.0` was discarded for being 17 days old — too new a major.

```
border: barreira 1 na borda right (2560, 0, 2560, 1440), eis=sim
border: vigiando borda 'right' -> windows-host
border: ACTIVATED barreira=1 activation=1 cursor=(2560.0, 555.18)
```

(Log: barrier 1 on the right edge, EIS = yes; watching edge 'right' ->
windows-host.)

### Three bugs only the live test showed

**1. solaar reports failure on a switch that worked.**

```
18:05:48  ACTIVATED
18:06:23  border: MX Anywhere 3: FALHOU --      ← EMPTY message, 35 s later
```

(`FALHOU` = failed.)

The devices were on Windows, channel 2: **the switch had
worked.** On a successful switch the device disappears in the middle of the
command, solaar hangs until the timeout and returns an empty error. This case
was already handled in the Rust agent (`"sem resposta é o caso ESPERADO"`, "no
response is the EXPECTED case") but had been missed in the solaar path.

Fix: **solaar's exit code is not a source of truth.** We verify
by the observable effect — if the slot left this receiver, it switched.

**2. The handler blocked the D-Bus loop for 35 s.**

The switch runs in its own thread now. Before, no signal was processed
during it.

**3. Giving the pointer back at the edge re-triggered the barrier in a loop.**

We got to `activation=54` with the cursor stopped at x=2560: each `Release`
put it back exactly on the barrier. `Release` accepts `cursor_position`,
so we pull it back **24 px** inward. After that, zero spurious
activations.

The cooldown went up from 3 s to 10 s, because the switch plus the effect-based
verification takes ~5 s and the second event came in before the first finished.

### The switch is not atomic

Mouse and keyboard are independent commands, seconds each. Observed:
the mouse left and the keyboard stayed for a few seconds — **a real and
transient state**, not a read error (the parser was suspected first, wrongly;
the API and `solaar show` agreed at the same instant).

The real risk is the second one failing and the split state becoming
**permanent** — and there is no rollback for the one that already left. That is why
`Outcome.partial` now shouts:

```
ESTADO DIVIDIDO: 1 trocaram, mas 1 ficaram aqui (MX Keys Keyboard).
Nao ha rollback para quem foi -- use o botao Easy-Switch no que ficou.
```

(SPLIT STATE: 1 switched, but 1 stayed here; there is no rollback for the one
that left — use the Easy-Switch button on the one that stayed.)

## The symmetric pair: `watch-edge` on Windows

[`agent/`](../agent/) gained `watch-edge`, which closes the mode B cycle:

```
oxdesk-agent watch-edge left 1,2 1 [hold_ms]
```

It watches the edge and, on touching it, sends the slots to the target's channel.

**On Windows this is much simpler than on Linux**: there is no portal,
no compositor barrier, no consent dialog. Reading
`GetCursorPos` and `GetSystemMetrics` does it. We use the **virtual** area
(`SM_XVIRTUALSCREEN` and family) instead of `SM_CXSCREEN`, which only reports
the primary monitor.

Design differences compared to the GNOME side:

| | Linux (`border.rs`) | Windows (`watch-edge`) |
|---|---|---|
| detection | compositor barrier via D-Bus | polling `GetCursorPos` every 30 ms |
| dwell | does not exist in mutter; we use a cooldown | `hold_ms`, default 250 ms |
| pointer pull-back | required (`Release` with `cursor_position`) | unnecessary: the cursor stops at the edge by itself |

### Dwell on the Linux side: activation density

Ported, but by another mechanism — mutter **has no** dwell, the barrier
fires on contact. We approximate it by **activation density**:

| | |
|---|---|
| `HOLD_WINDOW` | 400 ms |
| `HOLD_HITS` | 3 activations in the window |

The measurement that became a mechanism: with the cursor stopped at x=2560 we got to
`activation=54`, because each `Release` put it back on the barrier. That
re-firing is exactly the signal that the person is **insisting** on the
edge; a passing graze produces a single activation.

So `Release` became conditional:

- **while collecting** (hits < 3): it gives the pointer back **without** pull-back, on purpose, so
  that insisting keeps re-firing;
- **on deciding** (hits ≥ 3): it gives it back **pulled back** 24 px, resets the count and
  arms the 10 s cooldown.

The cooldown also comes before collection: inside it we do not even count, we only
give it back pulled back. This keeps the count from rising during the ~5 s of the
previous switch.

The inversion is the key detail: the churn first treated as a bug became
the intent signal.

### It needs the interactive session

Measured: running over SSH, the agent reports
`area virtual (0,0 1024x768)` — the fake screen of the non-interactive session,
not the real monitor. Same isolation that already invalidated `input-age` over
SSH. That is why the watcher **has** to be born in the interactive session.

> History: this used to be a shortcut in `shell:startup` pointing to
> `oxdesk-agent watch-edge`, created by a `windows-autostart.ps1`. Both
> were removed. The watcher became a **daemon thread**
> ([`border_win.rs`](../oxdesk/src/border_win.rs)) when `oxdesk` started
> running on Windows, because with both alive there would be two owners of the same
> HID++ receiver — it already happened, three simultaneous `watch-edge`
> tripling the radio traffic. The thread claims the **same mutex
> name** as the old mode precisely so that a forgotten shortcut never
> polls alongside the daemon.
>
> What guarantees the interactive session today is `oxdesk`'s scheduled task,
> created with `/it` by the wizard (`setup_win.rs`), and the dead script
> was still asking to create a shortcut to an `.exe` that no longer exists.

## The pointer pull-back does not exist: mutter ignores it

Measured in a real test, and this brought down the previous design:

```
300+ ativacoes, todas com cursor=(2560.0, ...)   <- x NEVER changed
690 linhas de "em cooldown, ignorando"
```

(300+ activations, all with the cursor at x=2560; 690 lines of "in cooldown,
ignoring".)

`Release` **accepts** `cursor_position` and **does not move the pointer**. With the
cursor resting on the edge, each Release put it back there and the barrier
re-fired — 300 times in one second, flooding the journal.

### The strategy that works: disarm the barrier

Instead of moving the pointer, `Disable()` on the decision and `Enable()` when the
cooldown expires. Without a barrier there is no event, regardless of where the
pointer is.

The re-arming `Enable` runs in the **main loop**, not in the switch thread:
the session belongs to that D-Bus connection, and another connection does not control it.
The shared state is a `reenable_at: Option<Instant>`.

> This also simplified `release()`: no options, no fallback. The
> whole function became three lines.

## `watch-edge` confused absence with failure

Reported from real use:

```
sem resposta para feature 0x00 func 0x00 ... switch_failed
```

(`sem resposta para` = no response for.)

Touching the Windows edge with the devices **on another machine**
made the agent try `set_host` and fail at `feature_index` — which is
absence, not failure. Now it checks presence first:

- no slot present → clear log and a 10 s cooldown, without trying;
- some present → sends only those and **warns about the split state**.

### Slots are per receiver, and do not match between machines

Discovered when gathering the devices: Windows had **slot 1 online**
and slot 2 empty, while on Ubuntu both exist. `set-host 2` from Windows
failed because of that, not because of a bug.

The `oxdesk` config already resolves by device **name**. The agent
still takes raw numbers on the command line (`watch-edge left 1,2 1`),
and the presence check makes it tolerate the divergence — but the right thing is for it to
read `DEVICE NAME` (HID++ 0x0005) and match by name too. Left open.

## solaar is too expensive for the hot path — measured

The switch **works**, but it took **105 seconds** in the real test:

```
18:51:18  border: barreira desarmada; trocando...
18:53:05  border: MX Anywhere 3: slot 1 saiu deste receptor (canal 2)
18:53:05  border: MX Keys Keyboard: slot 2 saiu deste receptor (canal 2)
```

(Log: barrier disarmed, switching...; each slot left this receiver, channel 2.)

Measured cause:

```
solaar show  ->  5,9 s  /  15,3 s  (second measurement under contention)
solaar show 1 ->  5,5 s            (per device, does not help)
```

And `change_host` calls that **4× per device** (one before, three for
verification), × 2 devices, plus `plan()`'s `survey()` and the SSH
to the agent. Hence the 105 s.

Aggravating: the **Solaar GUI runs in parallel** (`solaar --window=hide`),
and two owners of the same receiver is exactly the contention that the
OpenLogi docs describe.

### The way out: direct HID++, as on Windows

`hidapi` **works on Linux** (hidraw backend; needs
`libudev-dev`). And the enumeration showed more than expected:

```
pid=c52b  /dev/hidraw0  usage_page=0xff00   <- the receiver
pid=4090  /dev/hidraw2  usage_page=0xff00   <- MX Anywhere 3, on its own
pid=408a  /dev/hidraw3  usage_page=0xff00   <- MX Keys, on its own
```

Each paired device has its **own hidraw node** with HID++ collections.
Consequences:

1. It is possible to talk directly to the device (`device_index` 0xFF) instead
   of indexing a slot on the receiver — which **solves the numbering
   divergence** between machines.
2. "Is it online?" becomes a millisecond HID++ ping, not a 6-15 s
   `solaar show`.
3. The protocol code already exists (then in `agent/src/hidpp.rs`) and is
   platform-agnostic: it becomes a shared crate, today
   [`hidpp/`](../hidpp/src/protocol.rs).

That was the plan from the start ("the boundary of this module was designed
so that an HID++ 0x1814 implementation over /dev/hidraw replaces the
backend without touching the callers"). The measured cost of solaar made the
migration mandatory, not optional.

## Migrated to direct HID++ — measured before and after

`solaar` left the hot path. The protocol became a shared crate
in [`hidpp/`](../hidpp/), used by `oxdesk` **and** by `agent` — a single
copy, tested against hardware on both systems.

| | before (solaar) | after (HID++) |
|---|---|---|
| channel switch, end to end | **105 s** | **13.8 s** |
| confirmation that the device left | up to 4× `solaar show` | **300 ms** ping |
| `/api/devices` | 6-15 s | **1.5 s** |
| agent probe over SSH | 6.4 s | **2.06 s** |
| presence ping | — | **22 to 81 ms** |

### What HID++ gave as a bonus

**Name read from the device** (feature 0x0005), not from solaar. And the names
**diverge**: solaar said `MX Keys Keyboard`, the device calls itself
`MX Keys Wireless Keyboard`. The config had to be corrected — check
with `cargo run --release --bin probe` in the crate.

**Per-device latency** exposed in the API (`ping_ms`), which did not exist.

**Current channel** of each one, at no extra cost.

### Three defects the new speed revealed

**1. An offline device disappeared from the API.** HID++ does not read the name of
something that does not respond, so the list came out empty and the GUI blank —
a regression compared to solaar, which listed offline devices by name. Solved with a
slot→name cache persisted in `~/.local/state/oxdesk/slots.json`:
disposable runtime, not config.

**2. `executed: true` with zero results.** With the switch so much
faster, the edge watcher started winning the race against the API: the
pre-flight passed and, in the seconds until execution, another crossing had
already taken everything. Now switches are **serialized** by a global
mutex, and eligibility is re-evaluated **inside** the lock.

**3. The plan text still mentioned `solaar config`.** It now says
what it actually does: `HID++ 0x1814 setCurrentHost`.

### The tests changed subject

The 5 old tests covered the parser of `solaar`'s output, which no longer
exists. Instead of accepting the drop to zero, **7** were written on the
pure functions that remained: `barrier_line` on the four edges, with a secondary
monitor offset and an invalid edge; and config parsing —
`Expect` default, `down`/`any`, and the channel map by name.

### The remaining bottleneck

Of the 13.8 s, the pre-flight dominates, and within it the SSH to the agent. With the
agent migrated that dropped to ~2 s, so the switch should end up close to
**9 s**. The rest is SSH latency, not HID++.

## Keyboard follows the mouse: reconciler

[`oxdesk/src/follow.rs`](../oxdesk/src/follow.rs). One device is the
**leader** and the others in `[[devices]]` follow its channel, re-evaluated
every N seconds.

```toml
[input.follow]
enabled = false              # off by default
leader = "MX Anywhere 3"     # HID++ name
interval_seconds = 5
```

It solves the split state at the root, and — which the edge cannot do —
**works when the switch was manual, with the Easy-Switch button**. `border`
only sees cursor crossings; the reconciler sees the state.

### The hardware asymmetry drives the design

Only the holder of the device can push it, so the action depends on
where the follower is:

| follower is | who acts |
|---|---|
| here | us, via local HID++ |
| on a peer | we ask its agent, over SSH |
| on none we know | nothing — it may be the MacBook, with no daemon |

### Validated, on the hard path

The test exercised the direction that goes through the peer, not the local
one:

```
# only the leader goes back to channel 1; the keyboard stays on 2
ssh windows-host oxdesk-agent set-host 2 1 --confirm

follow: MX Keys Wireless Keyboard: pedi a windows-host que o mandasse para o canal 1
-> MX Anywhere 3              online=True canal=1
-> MX Keys Wireless Keyboard  online=True canal=1
```

(Log: "asked windows-host to send it to channel 1"; both devices then online
on channel 1.)

It acted **once** and settled down — more than that would be a loop.

### Match by name, not by channel

The first version located the device on the peer by comparing the **declared
channel**, and that is ambiguous: mouse and keyboard declare the same channel
for the same host. Fixed by having the agent report the HID++ name
(feature 0x0005) and matching by it.

The measurement that closed the argument, in the same test:

```
Ubuntu:   slot 1 = MX Anywhere 3            slot 2 = MX Keys
Windows:  slot 1 = MX Keys                  slot 2 = MX Anywhere 3
```

**Swapped.** Slots are per receiver and do not match — we already suspected it, and
now it is measured. Any logic based on slot numbers across
machines is wrong by construction.

### Safeguards

- **Off by default.** A reconciler that gets it wrong leaves you without a keyboard.
- **Never acts without knowing where the leader is.** Unknown channel = hands off.
- **Same mutex as the other switches** (`focus::switch_lock`): edge, API and
  reconciler never compete.
- **8 s wait after acting**, so it does not fight itself.

## Speed: 105 s → 1.3 s, in four measured steps

| | end-to-end switch |
|---|---|
| solaar | **105 s** |
| direct HID++ | 13.8 s |
| + parallelism and name cache | 3.9 s |
| + fast mode (peer cache) | **1.32 s** |

Each step came from a measurement, not intuition:

**1. Parallelize what does not depend.** The local scan (1.6 s) and the
agent probe over SSH (2.7 s) ran sequentially in `plan()`. Together in
`thread::scope`: 2.2 s instead of 4.3 s.

**2. Do not re-read what is already known.** `survey` read each
device's name over HID++ on every call — ~1.1 s of the 1.6 s spent
discovering what the cache already had.

**3. Scan only known slots.** The 6 slots cost ~1 s just in
timeouts of the 4 empty ones. With the cache, we scan 2 — and a full scan
every 10, otherwise a device paired later would never show up.
`/api/devices`: 1.63 s → **0.36 s**.

**4. Do not wait for SSH on the hot path.** The agent
probe was ~2.2 s of the 3.9 s. A background refresher
([`peers.rs`](../oxdesk/src/peers.rs)) keeps the peers' state every
6 s, and `"fast": true` reads the cache:

```
dry-run normal  2,193 s
dry-run rapido  0,197 s     <- 11x
troca rapida    1,32 s
```

(normal dry-run 2.193 s; fast dry-run 0.197 s; fast switch 1.32 s.)

The check **declares the age** of the data instead of pretending it is fresh:
`agente responde (cache de 7220ms atras)` ("agent responds, cache from 7220 ms
ago"). The edge watcher uses fast
mode; the API without `fast` keeps probing for real.

**5. `LEAVE_WAIT` from 300 ms to 120 ms.** The device leaves almost
instantly — the confirmation arrives on the first attempt.

## Battery over HID++, and a finding

Two features, because devices of the same family use different ones —
measured: the MX Anywhere 3 exposes **0x1004** (percentage) and the MX Keys
**0x1000** (range). We try the first and fall back to the second.

Raw bytes, so the parser is not an assumption:

```
slot 1  0x1004 getStatus   -> 05 01 00      (5%, level 1)
slot 2  0x1000 getLevel    -> 05 00 00      (5%, discharging)
slot 2  0x1000 getCapability -> 04 04 c0 06 0a   (critical = 0x0a = 10%)
```

Validated against an independent source: `upower`, which reads through the kernel's
`hid-logitech-hidpp` driver, reports **the same** — 5%,
`warning-level: critical`. Both devices are almost out of charge, and
that explains the "lost ping" previously attributed to power
saving: a device at 5% is slow to respond.

Battery does **not** go into the hot path (`survey_opts(false)`): it costs
two round trips per device and does not influence the switch decision.

## Button remapping (0x1B04)

Read from the reference deployment's hardware; the names match what Solaar
displayed:

```
slot 1 (MX Anywhere 3) -- 7 controles
  0x0050 Botao esquerdo        flags=0x01   nao reprogramavel
  0x0051 Botao direito         flags=0x01   nao reprogramavel
  0x0052 Botao do meio         flags=0x31   reprogramavel + divertable
  0x0053 Voltar                flags=0x31
  0x0056 Avancar               flags=0x31
  0x00c4 Smart Shift           flags=0x31
  0x00d7 Botao de gesto        flags=0xa0

slot 2 (MX Keys) -- 24 controles (teclas F e midia)
```

(Output: 7 controls on the mouse — left button and right button not
reprogrammable; middle button, Back ("Voltar"), Forward ("Avancar"), Smart Shift
reprogrammable + divertable; gesture button. 24 controls on the keyboard, F and
media keys.)

The flags come from the device itself: `0x10` = reprogrammable, `0x20` =
divertable. The left and right buttons report `0x01` — **without** the
`0x10` bit, and `set_control_remap` refuses with that justification instead of
trying and failing obscurely.

### API

| | |
|---|---|
| `GET /api/controls` | controls of each **online** device |
| `POST /api/controls/remap` | `{slot, cid, to, confirm}`; `to: "0"` restores the default |

It requires `confirm`, like the other actions that change hardware. Tested
refusals: without `confirm` → 400, device on another channel → 409, invalid
id → 400. An absent device does not appear in `/api/controls` —
listing a name without being able to read or write would be misleading.

### Write validated against hardware

Complete cycle, with the devices idle:

```
POST remap 0x0056 -> 0x0052    {"detail":"0x0056 (Avancar) -> 0x0052 (Botao do meio)","ok":true}
GET  controls                   0x0056 Avancar -> 0x0052 (Botao do meio)
POST remap 0x0056 -> padrao     {"detail":"0x0056 (Avancar) -> padrao","ok":true}
GET  controls                   todos -> padrao
```

(Forward remapped to the middle button, read back, restored to default
("padrao"), and all read back as default.)

The test caught two bugs before they reached real use:

**`to = 0` does not restore the default.** The write returned `ok` and the
device kept reporting the old remapping. In 0x1B04 the
default is restored by remapping the control **to itself**. Now
`to: "0"` is translated to the `cid` itself.

Trusting the `ok` return instead of reading back would have left the
Forward button altered while reporting success.

**The ping gate needed a retry.** `remap` failed with
`slot 1 nao responde` ("slot 1 does not respond") seconds after a successful read.
A retry existed in `survey` but had been left out of `remap` and `change_host`
— with the devices at 5% battery, a single ping is lost. All three
now retry (3 to 4 times, 180 ms apart).

### Observed limit: low battery breaks long enumeration

The keyboard has 24 controls, and at 5% it cannot sustain the ~50 round
trips: `/api/controls` returns
`MX Keys Wireless Keyboard (erro: sem resposta)` ("error: no response")
while the mouse, with
7 controls, completes. The error appears per device instead of
bringing down the whole response.

## Why the Windows edge "did not work" — a presence-check bug

Reported symptom: hitting the edge on Windows sometimes did not bring the
devices. The agent's log — which exists only since logging was added on
that side — showed this 25 times in 5 minutes:

```
21:24:54  BORDA TOCADA em (0,516) mas nenhum dos slots [1,2] esta neste receptor
21:25:06  BORDA TOCADA em (0,516) mas nenhum dos slots [1,2] esta neste receptor
...
```

(EDGE TOUCHED at (0,516) but none of slots [1,2] is on this receiver.)

The edge **was detected** — the cursor position is right there. What failed
was the **presence check**, saying there was no device
while the mouse was being moved on that machine. That is impossible.

Cause: presence was checked with `host_info`, which costs **two** round
trips with a 700 ms timeout each, and **no retry**. With the devices at
5% battery, that gets lost. Exactly this had already been fixed on the
Linux side and was missing in the agent — the second time the same oversight
showed up.

Now there is `Receiver::present(slot, tentativas)` (`tentativas` = attempts) in the shared
crate, with `ping` (250 ms) and retry, and both sides use it. One
retry implementation, not two.

## `watch-edge` guards

Real problem observed: **three instances at the same time**, competing
for the same receiver and tripling the HID++ traffic. The Startup shortcut
plus a manual launch stack up.

Two guards, each of which needed a second attempt:

**Console session.** Launching over SSH creates an instance in session 0 that
sees no input at all (`area=(0,0 1024x768)`, idle equal to the uptime) and still
contends for the receiver. `WTSGetActiveConsoleSessionId` +
`ProcessIdToSessionId` refuse this with an explicit message.

**Single instance, in the `Local\` namespace.** The first version used
`Global\`, and **two instances got through**: `Global` requires
`SeCreateGlobalPrivilege`, and when creation fails there is no way to
distinguish "already exists" from "could not create". Since the session guard already
blocks the cross-session case, the per-session namespace is exactly the scope
that was missing.

> Method note: during testing, a count of two processes nearly led to reporting
> the guard as broken. They were the good instance plus the refused one **exiting** —
> the measurement was racing the teardown. The log had a single
> `watch-edge START`, and that is what decides, not `Get-Process`.

### Deploying on Windows needs a rename, not a direct `scp`

`scp` over the exe fails with `dest open ... Failure` when any
instance keeps it open, and the script still printed "trocado" ("swapped"). Now:
send it as `.new.exe`, `Remove-Item` the current one, `Move-Item`, and
**check size and date** — do not trust the absence of an error.

## The unnecessary wait: 2.27 s → 66 ms

A delay was still reported after fast mode, and the report was right.
Timed in isolation, without the API or verification:

```
abrir receptor     :  0,01 ms
ping de posse      : 45,54 ms
feature index      : 20,00 ms   (memoized after the 1st time)
escrita ChangeHost :  0,60 ms
TOTAL              : 66,14 ms
```

(open receiver 0.01 ms; ownership ping 45.54 ms; feature index 20.00 ms;
ChangeHost write 0.60 ms; total 66.14 ms.)

In the daemon `feature_index` is already memoized, so the real cost is
**~46 ms**.

### What the rest was

**`set_host` waited 700 ms for a reply that never comes.** On a
successful switch the device leaves the radio **before** replying — this
was already known (absence was treated as success), yet the code still paid
the whole `REPLY_TIMEOUT`, on each device. `set_host_nowait` writes the
report and returns: **0.6 ms**.

**`feature_index` cost a round trip per call.** It is stable
per device; memoized, it leaves the hot path (20 ms → ~0).

**`plan()` scanned the receiver on every crossing** (~360 ms). Now there is a
short cache, kept at 1.5 s by a refresher, with a fallback to a
scan if it is stale — correctness before speed.

**The two switches were sequential.** Now they fire in parallel, and the
confirmation became a background log (`verificacao: slot 1 saiu em
120ms`, "verification: slot 1 left in 120 ms") instead of blocking the response.

Measured: `plan()` in fast mode responds in **0.98 ms**.

### The tension this creates: a usage decision

The dwell was lowered from 3 touches in 400 ms to **2 in 300 ms**, and made
configurable:

```toml
[input.border]
hold_hits = 2          # 1 = switch on the first touch
hold_window_ms = 300
```

But crossing the edge in oxdesk **is not like Flow**: there the cursor comes back
for free, here it is a radio re-pairing. With `hold_hits = 2` the
normal movement up to the right edge already fires — observed during the
tests, the devices left three times unintentionally.

`hold_hits = 1` gives the immediacy that was requested, at the cost of switching
machines on a graze. `3` avoids the accident and brings back the delay. There is no
right choice independent of how the edge is used.

## Battery status is the device's claim, not ours

Reported symptom: the keyboard had been charging for 30 minutes, yet the panel
said `descarregando` ("discharging"). Investigated in the raw bytes:

```
MX Keys   0x1000 getLevel -> 32 14 00    = 50%, next 20%, status 0
MX Anywhere 3 0x1004 getStatus -> 05 01 01 01  = 5%, level 1,
                                                 chargingStatus 1, externalPower 1
```

**Charging was working**: the keyboard had gone up from 5% to 50%, and
the mouse went from 5% to 70% during the investigation itself. What was
wrong was the panel's label.

The MX Keys reports `status = 0` ("discharging") **on the cable, with the charge
rising**. `upower`, which reads through the kernel driver, reports the same — the
false claim is the firmware's. But the panel displayed it as if it were oxdesk's
own reading, in a field called `battery_level`, as if it were the level.

Fixes:

- `battery_level` became **`battery_status`**, and the docs say whose
  claim it is. The tray shows `diz: descarregando` ("says: discharging"), not `descarregando`.
- `charging` for 0x1004 now uses `chargingStatus` **or**
  `externalPower` (byte 3), more reliable than the first alone.
- `battery_next_level` (byte 1 of 0x1000) is no longer discarded.

> When the device is absent, the value comes from the cache with
> `battery_age_s` — observed: keyboard `50% idade=49s` while the mouse
> read `70% idade=0s` ("idade" = age). Memory declared as memory.

## Immediate switch at the edge, on request

`hold_hits = 1`: switch on the first touch, no dwell. Chosen explicitly
for the reference deployment after weighing the trade-off above — there,
the right edge is not a frequent path.

The repo default stays `2`, so whoever installs it is not surprised.

## Deskflow still showed up in the panel

Removed from the package and from both machines, but it remained in the config: a
`[[hosts.services]]` on port 24800 and the `deskflow-client.service` unit
in `units`. The panel showed `Deskflow srv: fechada` ("closed") and
`deskflow-client · inactive` — noise about something that no longer exists.

Replaced by `oxdesk.service`, which is what actually matters to watch.

## When the devices "switch hosts on their own": the list (2026-11-04)

This took two mornings of investigation. The next person deserves a
checklist instead of an investigation.

**Who can move a device, and none of them show up if you look
only at one machine's panel:**

| Actor | Where | How to turn off | Gotcha |
|---|---|---|---|
| Edge (GNOME) | `border.rs`, Ubuntu | `input.border.enabled` | Barriers are created at boot; the flag is now read **on touch** |
| Edge (Win32) | `border_win.rs`, Windows | the same flag, **on that** machine | Different file — fixing one does not fix the other; this was missed once |
| Dead-man's switch | `border_win.rs`, `Guard` | together with the edge | Gives everything back if nobody types within 25 s |
| Reconciler | `follow.rs` | `input.follow.enabled` | Asked the peer and believed it without checking; today it has a circuit breaker |
| USB power | **Windows, outside our code** | see below | **Logs nothing anywhere** |

### The actor that leaves no trace: USB power saving

With edge and reconciler off on all three machines and no log line
claiming the action, the devices still went back to channel 1
a few seconds after going to 2. No software asked for it.

Windows had permission to **turn off the Logitech receiver** to
save power — on its three interfaces and on the root hub:

```powershell
Get-CimInstance -Namespace root\wmi -ClassName MSPower_DeviceEnable |
  Where-Object { $_.InstanceName -like '*VID_046D*' } |
  ForEach-Object { Set-CimInstance -InputObject $_ -Property @{ Enable = $false } }
```

Suspended dongle = the device does not find host 2 and falls back to 1,
which keeps calling. It explains both symptoms at once: "sometimes it
doesn't go" (dongle asleep when the request arrives) and "when it goes,
a few seconds later it comes back".

### The investigation order that works

1. `GET /api/status` on **any** machine: each host's `kvm` field
   tells the edge and reconciler **of that** host. It was added
   precisely because looking at one side only was not enough.
2. Logs from **both** machines. "No line claims the action" is
   information, not a lack of information: it rules out our code.
3. Processes separate from the daemon: `oxdesk-agent watch-edge` has its own
   shortcut in Windows Startup and does not die with the daemon's flag.
4. Another manager: `solaar`, `logiops`, `piper`, `ratbagd`.
5. USB power on the DESTINATION host.

### The cause, at last: the Windows receiver on a USB 3.0 port (2026-11-04)

After ruling out edge, reconciler, probing (paused on both
machines), Options+, USB power and pairing -- all by experiment,
none by guess -- the missing measurement remained:

| channel | receiver on | |
|---|---|---|
| 1 (Ubuntu) | bus `usb1`, 12 Mbps, version 2.00 | **USB 2.0** |
| 2 (Windows) | `USB\ROOT_HUB30` | **USB 3.0** |

A USB 3.0 controller emits broadband noise around 2.4 GHz, which is
the Unifying band. A dongle on a USB 3 port loses range and drops
intermittently -- documented by Logitech itself.

**What this explains, which nothing before explained:**

- The devices always go back to **channel 1**, not to a MACHINE.
  When the Ubuntu receiver was moved to Windows, the devices
  converged to channel 1 **on Windows**. It is not "something pulls toward
  Ubuntu"; it is "they drop host 2 and fall onto the stable host".
- The relationship with the battery, which was the piece that did not fit: more charge
  = stronger transmitter = survives longer in the noise. Measured, with the
  same software and the same receivers:

  | battery | time on channel 2 |
  |---|---|
  | keyboard at 0% | ~10 s |
  | keyboard recharging | ~48 s |
  | mouse at 85% | ~85 s |

- No log line claiming the action, on either side, at any
  moment. It was information, not a lack of information: it ruled out the software.

**Remedy:** move the Windows dongle to a **USB 2.0** port, or use
a short extension cable to get it away from the case and from the vicinity of the
other USB 3 ports.

**The process lesson:** six software actors were investigated before
anyone measured which port the receiver was on. The investigation order at the start
of this section now begins with the physical -- speed and bus of both
receivers -- because it costs one command and rules out the whole class.

### The stickiness follows the dongle, not the machine -- and dongle B reads garbage (2026-11-04)

Swapping the two dongles between the machines, the stickiness followed the **dongle**,
not the machine: the devices went to wherever dongle A was, and
stayed there stable for minutes -- on the same machine and the same USB
3.0 ports that had been blamed. **That brought down the USB
3.0 noise hypothesis**, which had been built on a battery ladder that seemed
to confirm it.

With dongle B on Ubuntu, the HID++ reads that go through it came back
**corrupted, repeatedly**:

```
MX Keys WirelessMX Keys W    canal=5 de 19
```

The name came concatenated with itself and the channel came as **5 of 19** -- on a
three-host keyboard. Two reads in a row, the same garbage. A receiver that
returns a malformed payload is not weak, it is defective.

**What this explains:** the devices dropped off dongle B within tens of
seconds; the battery-duration ladder was real because any bad link
behaves like that; and the "always on host 1" convergence was convergence on the
**dongle that works**.

**Remedy:** use only dongle A, and reach the other machine through the remote
screen (specs/010) instead of moving the devices.

#### Caveat: the title of this section claims more than was measured

"Dongle B is defective" was written on the same day the garbage was seen,
and commit cb5722c carries that sentence. It may be right, but the two
corrupted reads **do not prove it**: at that moment the devices
were dropping off dongle B all the time, and an HID++ read from a
device that has already gone off the air returns exactly this kind of nonsensical
payload. Garbage in the response is a symptom compatible with a defective receiver
**and** with an absent device -- and both conditions held at the
same time.

What was measured, and is enough for the practical decision:

- the stickiness follows the dongle, not the machine nor the port (that brought down the
  USB 3.0 hypothesis, and was measured by swapping the two);
- with dongle A alone the reads are coherent -- `canal=1 de 3` on
  both devices, correct name, stable over 20 minutes of watching;
- with dongle B the reads were not reliable.

That justifies using only dongle A. It does not justify asserting the physical cause.
To decide for real it would be enough to pair both devices on dongle B
on a machine without dongle A nearby and read with them present and
awake; if the garbage comes back with a live device, then it is the receiver.
Noted as not done, not as known.

### Two instruments fixed because of this

1. **`host_info` validates the range.** A channel above the total the device itself
   declared became an error, not a number. Before, it returned `5` as
   fact and the reconciler acted on it. Three tests cover the rule.
2. **The name cache can get poisoned.** A corrupted read wrote
   `MX Keys WirelessMX Keys W` to disk and the panel kept repeating it
   even with the good dongle. Clearing it fixes it, but the lesson is that a cache of
   data read from bad hardware needs validation on INPUT -- today it
   accepts whatever comes.

### The rule that comes out of this

**A switch that does not turn off is worse than no switch at all.** A resident
loop has to read the flag on every iteration, not at boot — otherwise the screen
shows "off" over a live behavior, and the person loses trust
in the panel exactly when they need it most. It applies to the edge on both
platforms, to the reconciler and to anything we
add here later.

## Open

- The Ubuntu machine's channel: one Easy-Switch keypress resolves it.
- Edge detection through the `InputCapture` portal — the API exists in GNOME 50
  (the backend declares it and `CreateSession` is accepted), but it is D-Bus + libei, and
  v1 may not be enough.
- `guard` does not run resident yet: it needs a shortcut in `shell:startup`
  to stay in the interactive session, since `GetLastInputInfo` does not see input
  from another session.
- Real end-to-end latency of the HID++ switch — measure before promising.
- What if the mouse is on the channel of a machine that is off? Only the physical
  button solves it, and the GUI has to say so.

## KVM over LAN: switching machines without switching dongles (2026-11-04)

Two dongles close together do not work — the stickiness follows the dongle, and only one
stayed, on Ubuntu. That solves the instability and leaves a problem: reaching
the other machines requires moving the receiver by hand.

The way out is to stop treating a machine switch as a radio command to the
keyboard and start treating it as a software decision: capture here and
inject there, over the LAN.

### Why evdev and not Mutter's `InputCapture`

GNOME 50 exposes `org.gnome.Mutter.InputCapture`, which is the API made
for this. Introspecting a session (a throwaway probe, because it
dies together with the connection that creates it and `busctl` does not work), it
offers:

```
GetZones, AddBarrier, ClearBarriers, Enable, Disable,
ConnectToEIS, Release, Close
signals: Activated(barrier_id, activation_id, cursor_position), Deactivated
```

There is **no** method to activate capture on demand. The
compositor only delivers input when the pointer crosses an edge
barrier — exactly the gesture we are trying to abandon in favor of the
tray click.

It could be worked around: create a barrier and teleport the pointer
into it via `RemoteDesktop`. That would be simulating the edge, with more parts
to break.

evdev solves it directly: `EVIOCGRAB` is explicit (the click's
semantics), **the codes are already ours** — `Evento::Tecla` carries an
evdev keycode because that is what `NotifyKeyboardKeycode` receives, and
`scancode_from_evdev()` already translates it for Windows — and it is independent of the
compositor.

### Relative movement

`Evento::MouseRel` entered the protocol. The viewer sends absolute
because it knows where the cursor is inside its window; the KVM has no
window. And games read relative movement: with absolute the crosshair gets stuck at the
edges, and "switch to Windows to play" is the case that motivated everything.

The new tag almost activated a latent bug: the decoder ended in
`_ => Tecla`, so any unknown tag read 5 bytes. A 16-byte `MouseRel`
read as a key would leave 11 loose bytes and, from then on, the whole
stream turns into phantom commands **on someone else's computer**. Each
tag has its own arm now, and a test locks the format.

### The three safeguards

Grabbed, the keyboard does not exist for the local desktop. That is what
we want — without the grab, each key would be typed on both computers
— and it is what makes the mode dangerous.

1. **Connect before grabbing.** If the connection fails, nobody grabs.
   Without this, a target that is down would leave the person without a keyboard and without a
   destination.
2. **An escape hatch that does not depend on the network.** Both Ctrl keys together release everything
   and exit. No text produces them and no program uses them — it is the
   same gesture as a hardware KVM switch, for the same reason.
3. **Release what was left pressed.** The kernel releases the grab when the fd
   closes, so a keyboard stuck forever is not reachable. But the
   keys pressed **on the other side** would stay stuck, and that is
   reachable — it already happened on the viewer path. We send
   key-up for everything before exiting.

`--observar` ("observe") reads and prints without grabbing and without sending: it is the way to
check before letting the program steal someone's keyboard.

### Permission

`oxdesk/udev/70-oxdesk-input.rules`, with `TAG+="uaccess"` on the
Logitech devices — not the `input` group, which would give any
process of the account permanent access to every input device,
even with the session closed. Recipe in
`.claude/skills/grant-input-access.md`.

### What is missing

- The native tray that calls this (today it is command line).
- Capture on Windows (`SetWindowsHookEx` + Raw Input) for the reverse
  path, when the dongle is there.
- Input-only mode on the host: today the KVM connects to the screen host, which then
  encodes video with nobody watching. For gaming on Windows that
  is wasted GPU.
