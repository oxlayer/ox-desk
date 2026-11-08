# 001 — Control plane (`oxdesk`)

Implemented in Rust in [`oxdesk/`](../oxdesk/). The Python prototype
(`monitor.py`) was replaced in phase 1 — see
[`000`](000-overview.md#language-rust).

## Symmetric: every node is a peer, none is central

**Decision of 2026-11-03, by the user:** with the daemon running, *any*
machine can become the host; what decides is where the person is sitting.

The first version of this spec said "on Ubuntu it runs as the control plane,
on Windows as an agent". That was wrong for two reasons:

1. **The hardware already imposes symmetry.** Only whoever holds the device can
   issue `ChangeHost` ([`005`](005-input-switch.md#discovered-constraint-only-the-holder-of-the-device-can-push-it)).
   A central node that does not have the device cannot move anything — it can
   only *ask*.
2. **There are three machines**, not two: Ubuntu, Windows and a MacBook on
   channel 3. Electing one as central is arbitrary.

### The model

Every node runs the **same** `oxdesk`. No fixed role. The state that
matters is derived, not configured:

| Question | Who answers |
|---|---|
| who holds the devices? | each node reads its own receiver; exactly one sees `online=true` |
| who is the current host? | the holder |
| who can switch? | **only the holder** |

Asking for focus is therefore a **forward**, not a command:

```
me (any node)  --asks-->  current holder  --ChangeHost-->  target
```

If I am already the holder, the forward is local. If I am the target,
I am asking to be handed the device. If I am neither, I am just the
intermediary — and that is legitimate: the GUI on any machine must be able
to move focus between the other two.

### Transport between peers: SSH

It stays SSH, for symmetry and because it already works
([`002`](002-ssh.md)): no new port, no new auth, and macOS already ships
sshd. **Open item:** Ubuntu does not have sshd yet — today it only dials
out. For the symmetry to hold, `openssh-server` here goes into the
bootstrap.

> Discarded alternative: our own LAN protocol with mTLS. It would mean
> reimplementing what SSH already delivers, and multiplying surface — see
> the criterion in [`000`](000-overview.md#what-we-build-and-what-we-integrate).

### What ceases to exist

- The concept of "agent" as a separate binary. The current
  [`agent/`](../agent/) is an intermediate step, not the destination:
  the target is **one binary per platform, same role**.
- `[[hosts]]` with a node marked as local for having `addr = 127.0.0.1`.
  Each node knows who it is by hostname; the others are peers.

## The same binary on both OSes

This spec had claimed "every node runs the **same** `oxdesk`" since 2026-11-03. It was
an aspiration: the daemon only compiled on Linux. Windows had
`oxdesk-agent`, a one-shot CLI, with no API, no tray, no window — and
the Ubuntu GUI reached the other machine over SSH, calling the CLI by hand.

The user noticed through the gap this leaves: *"why is there no GUI and
taskbar shortcut on Windows, only here on Ubuntu?"*. The answer is that the
**switch** became symmetric (`watch-edge` gave Windows the edge) but the
**control plane** did not. Whoever was sitting at Windows had no panel.

### What actually tied it to Linux

Measured before deciding, module by module. Of the nine, **six are
portable as they are** (`agent`, `config`, `focus`, `follow`, `hidpp`,
`peers`). Five points tied it down:

| point | Linux | Windows |
|---|---|---|
| DNS fallback | `getent hosts` | nothing — the resolver already speaks mDNS |
| `units` state | `systemctl --user is-active` | service, otherwise process |
| local time | `date +%z` + arithmetic | `GetLocalTime` |
| log source | `journalctl --user -u` | file in `%LOCALAPPDATA%` |
| HID access | `uaccess` ACL on `/dev/hidraw` | open the receiver |
| edge detection | `Mutter.InputCapture` barrier | `GetCursorPos` |
| config directory | `~/.config/oxdesk` | `%APPDATA%\oxdesk` |

Hence [`oxdesk/src/sys/`](../oxdesk/src/sys/): one place for whatever diverges,
instead of a second daemon in `agent/` duplicating the other six
modules. The alternative — growing the agent until it became a daemon — would cost
two copies of `focus`, `follow` and `peers` that would have to agree.

### Dwell at the edge is expressed differently on the two ends

It is not an inconsistency, it is the available signal. Mutter delivers discrete
events, so on Linux dwell is **density**: `hold_hits`
activations within `hold_window_ms`. On Windows we measure **continuous**
contact, which is what polling at 30 ms actually sees. The translation:
`hold_hits <= 1` switches on the first touch; above that it requires staying at the
edge for `hold_window_ms`.

### `watch-edge` became a thread, and claims the same mutex

With the daemon on Windows, the separate `watch-edge` process stops
making sense — and becomes *dangerous*: two owners of the same HID++
receiver is exactly what the single-instance lock exists to prevent
(it has happened: three simultaneous `watch-edge` tripling the radio
traffic).

That is why [`border_win.rs`](../oxdesk/src/border_win.rs) claims the
**same mutex name** as `watch-edge`, `oxdesk-agent-watch-edge`. It is not
a coincidence: it is what guarantees that the old shortcut and the new daemon never
poll together, even if someone forgets to remove the shortcut from
`shell:startup`.

The edge thread is also the only one that gives up outside the console session
— `GetCursorPos` and `GetLastInputInfo` are per session, and in session 0 (the
case of launching over SSH) it would read a fake 1024x768 desktop and an idle
time equal to uptime. The rest of the daemon works there; only the edge does not.

### "Who am I" started needing an answer

It was implicit — `addr = "127.0.0.1"` and free text in `role` saying
"this PC". The Windows dead-man's switch needs to know where to
return the device to, and "the host that is not me" requires knowing who I
am. Hence `local` in `[[hosts]]`, optional, with the loopback heuristic
as a fallback for configs that already exist.

While writing this, a second reading of the same criterion showed up:
`follow.rs` had its **own** copy of `is_local`, which ignored the new
flag. On a Windows box with `local = true` and a non-loopback address, `follow`
and the edge would disagree about which machine is "me". The copy was removed and
`who_am_i_by_loopback_or_by_flag` is the test that prevents it from
coming back.

### How it starts on Windows

It is not a service. A service runs in session 0, where `GetCursorPos` and
`GetLastInputInfo` see nothing — and the edge will need the console
session. It is a **scheduled task** with `/IT` (*interactive only*), which runs
in the user's session at login and does not ask for a stored password:

```powershell
schtasks /create /tn oxdesk /tr "%LOCALAPPDATA%\oxdesk\oxdesk.exe" /sc onlogon /it /f
```

Verified: `pid 2356 sessão 1`, listening on 8787.

Before that I tried `Start-Process` over SSH. It started, answered `healthz` —
and died together with the SSH session, because Windows OpenSSH kills the
process tree on disconnect. It is the same principle as the
`long-running-jobs` rule in the other repo: resident work does not live hanging
off a session that goes away.

The binary is `windows_subsystem = "windows"`: without it a console window
appears at every login and every time the GUI starts the daemon. It does not
affect the JSON over SSH — the subsystem decides whether a console is *allocated*, not
where stdout goes; with a pipe from the parent, the output keeps arriving
(verified with `Start-Process -RedirectStandardOutput`).

The GUI is the same Electron as on Ubuntu, in `%LOCALAPPDATA%\oxdesk\gui`, with
two shortcuts — Start Menu (pinnable to the taskbar) and `Startup` — pointing
to `electron.exe .` with a multi-resolution `icon.ico`. The paths that
change per OS are in `paths()` in [`gui/main.js`](../gui/main.js), and must
match `sys::config_dir`: diverging there means the GUI reads another
config and does not find the daemon.

> `npm install` did **not** download the Electron binary (13 packages in 5 s,
> no `dist/`), despite `ignore-scripts = false`. Running `node
> node_modules/electron/install.js` by hand fixed it. If the GUI there
> stops opening after an `npm ci`, that is the first place to look.

### Session isolation also fools whoever is verifying

I spent several rounds convinced that the GUI window did not open:
`MainWindowHandle` came back `0` for the four Electron processes, and my
`EnumWindows` found no window at all. The conclusion was wrong.

`EnumWindows` only enumerates windows of the **window station of the caller's
session**, and my SSH is in session 0. `MainWindowHandle`, from .NET, uses the
same mechanism. It was not the window that did not exist — it was me looking from
the wrong place, exactly the trap this spec already documented for
`GetCursorPos`, now applied to *verification*.

What actually proved the GUI works was the renderer's stderr: a
CSP warning coming from **our** page, which only happens after it
loads. And the warning was a real finding — the panel did not declare a
Content-Security-Policy, which is equivalent to allowing `unsafe-eval`. The
page uses no `eval` at all (checked), so the policy was declared
and the warning went away.

Operational lesson: when verifying something in the interactive session from a
non-interactive session, prefer signals that cross the boundary —
log file, TCP port, process — over window APIs, which do not
cross it.

### The symmetry closed (2026-11-03)

`focus::plan` **blocks** the switch when the target lacks `ssh` + `agent`,
because without someone on the other side able to return the device, the origin
cannot undo it ([`005`](005-input-switch.md)). `watch-edge`
escaped this by not going through `focus` — it worked, but without the gates.

Two pieces were missing, and neither was loosening the check:

1. **`oxdesk` also became the "agent" of its own host.** `oxdesk devices`,
   `host-info`, `set-host` answer a line of JSON, same as
   `oxdesk-agent`. The format lives in
   [`hidpp/src/protocol.rs`](../hidpp/src/protocol.rs), shared
   by both binaries: it is the contract *between the machines*, the worst
   possible place for two copies. (It was `survey()` inside the agent; the second
   copy would diverge and show up as "agente nao respondeu JSON" in a log,
   far from the cause.)
2. **`sshd` on Ubuntu**, via [`scripts/enable-sshd.sh`](../scripts/enable-sshd.sh),
   key-only and for a single user.

Verified in the dry run (`POST /api/focus` with `confirm: false`) **from the
Windows side**:

```
[ok  ] dead-man's switch no alvo
       agente responde; dispositivos la: slot 1 (canal 1/3), slot 2 (canal 1/3)
[BLOQ] posse do dispositivo
       'MX Anywhere 3' esta offline aqui -- ja esta em outro canal
```

The remaining blocks are the **right** ones: only whoever holds the device can
push it, and at that moment it was on Ubuntu.

With that, `watch-edge` was retired — shortcut removed from
`shell:startup`, processes terminated — and `[input.border]` turned on on
Windows. The reconciler (`[input.follow]`) was left **off** there on
purpose: it is one reconciler, not two. Ubuntu's already covers both
machines, acting directly when the device is there and asking over SSH
when it is here; turning both on would make both reach the same conclusion and
act twice.

### The host needs to declare its OS

Two places build a command to run **on the peer**, and both were
Windows-specific: `agent::devices` builds PowerShell (`& "exe"
devices`) and `probe::ssh_detail` runs a one-liner with CIM. Against Ubuntu
or macOS both would fail — and fail **silently**, because the
detail is decoration and the agent probe already degrades to "did not respond".

Hence `os = "windows" | "linux" | "macos"` in `[[hosts]]`, with inference
for configs that already existed. On Unix the command goes with single quotes
and without the `&` — which there would mean *background* and return an empty
success, read as "agente nao respondeu JSON".

## API

Prefix `/api`. JSON everywhere. Always on `127.0.0.1` (see
[`008`](008-security.md#why-localhost-only)).

| Method | Route | Purpose |
|---|---|---|
| `GET` | `/healthz` | alive? Does **not** touch the network — it exists because the 800 ms health check was hitting `/api/status`, which takes 2 s. A real mistake already made. |
| `GET` | `/api/hosts` | machines, state, services |
| `GET` | `/api/hosts/{id}` | detail (uptime, RAM, HID devices) |
| `POST` | `/api/hosts/{id}/focus` | **switch focus to this machine** — see [`005`](005-input-switch.md) |
| `GET` | `/api/config` | effective config + where each value came from |
| `PATCH` | `/api/config` | change; validates before writing |
| `POST` | `/api/bootstrap/{id}` | provision the other machine over SSH ([`002`](002-ssh.md)) |
| `GET` | `/api/files/{id}?path=` | list directory ([`003`](003-files.md)) |
| `GET` | `/api/clipboard` / `POST` | read/push clipboard ([`004`](004-clipboard.md)) |
| `GET` | `/api/events` | SSE: state changes, so the GUI does not have to poll |

## State

Two levels, separated on purpose:

- **Config** — what you chose. A versionable, readable file,
  editable by hand: `~/.config/oxdesk/config.toml`. We never write
  secrets here.
- **Runtime** — what was discovered. Disposable cache in
  `~/.local/state/oxdesk/`. Deleting it loses nothing beyond a re-read.

Secrets (SSH key, TLS cert) go into **neither**: they stay
where the owning tool already keeps them (`~/.ssh`, `~/.config/Deskflow/tls`) and
we reference them by path.

## Adapters

Each integration is a module with the same shape: `probe()`, `read()`,
`apply()`. No external tool called directly from a handler —
same reason as the [pure functions](../CLAUDE.md) rule in the other repo: the
handler resolves IDs, the adapter does I/O, the decision is a testable pure
function.

| Adapter | Wraps | Notes |
|---|---|---|
| `ssh` | OpenSSH client | dedicated key, `BatchMode`, agent for the passphrase |
| `sftp` | OpenSSH | file browser; no new dependency |
| `deskflow` | `deskflow-core`, `Deskflow.conf` | **server segfaults on Linux** — client mode only |
| `hidpp` | Solaar / HID++ 0x1814 | Easy-Switch channel switching |
| `kdeconnect` | GSConnect DBus / `dconf` | clipboard, share, notifications |
| `rustdesk` | RustDesk binary | screen; lifecycle only, not protocol |

## GUI

The Electron shell in [`gui/`](../gui/) already handles
window, tray, single instance and reuses the systemd backend.
It **does not duplicate logic**: every rule lives in `oxdesk`.

Recorded trap: the VS Code extension host exports
`ELECTRON_RUN_AS_NODE=1`, and with it the Electron binary runs as plain
Node and nothing opens. `run.sh` and `npm start` clear the variable.

## Principles

1. **Idempotent.** Every `apply` route can be called twice.
2. **Say where the value came from.** `/api/config` returns the origin along
   with the value — default, file, or discovered.
3. **Never write an IP.** See [`007`](007-device-routing.md) and the
   *"O IP muda"* ("The IP changes") section of the README.
4. **Fail loudly.** The other repo's `langfuse` taught us: a `catch`
   that swallows an error costs hours. If an adapter did not find the tool,
   that shows up in the API and in the GUI.
