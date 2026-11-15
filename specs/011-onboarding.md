# 011 — Setting up a machine

> Status: implemented (Ubuntu and Windows measured; macOS written and
> **not verified** — the MacBook was offline that day).

## The problem

Setting up a machine in this set is a list of eight to eleven things,
different on each system. What makes the list expensive is not its
length: it is that **none of the items gives a clear error when it is
missing**.

| What is missing | What the person sees |
|---|---|
| ACL on `/dev/hidraw` | the panel finds no device at all |
| Screen Recording (macOS) | the log stops at `abrindo a tela 1` ("opening screen 1") and the last line talks about pixel format |
| Accessibility (macOS) | the screen appears and the pointer does not move |
| Task without `/IT` (Windows) | `E_ACCESSDENIED` or a black capture |
| Key in `~/.ssh` instead of `administrators_authorized_keys` | `Permission denied`, no clue |
| Ad-hoc binary on the Mac | everything works until the next deploy, and then stops |

All the symptoms above arrive as "it didn't work". `status.sh` already
answered part of this, but only on Linux, only in the terminal, and only
for those who know it exists.

## The shape

A page served by the daemon itself at **`/setup`**, which measures what
is missing on this machine and fixes what can be fixed. The Electron
shell opens it full screen at startup **when something essential is
missing**, and the tray has the item "Set up this machine".

It runs **from inside the daemon**, and that is half the design: the
daemon lives in the graphical session (LaunchAgent on the Mac, `/IT` task
on Windows, user unit on Linux), so it sees what an SSH session does not.
Measuring over SSH would give "no" for things that are right — see
[[../.claude/rules/graphical-session]].

## The three rules

### 1. Not measured is a state, and it blocks

`State::Error` exists alongside `Ok`, `Missing`
and `NotApplicable`, and weighs the same as `Missing` in the
summary. A step the wizard could not measure **is not an approved step**.
A step nobody measured is not either: `state_of` returns `Error` for a
missing id, and there is a test for that.

This is the same family of defect as a feature flag that is documented
and switched on but read by no code: the silent no-op looks like success.

### 2. Fixing is not having fixed

Every repair **measures again** and returns what the measurement said,
not what the command returned. The Mac's `sign.sh` is deliberately
non-fatal; `schtasks` returns 0 while creating a task without an
interactive token; `electron` starts and dies within 200 ms. In all three
cases the exit code lies and the measurement does not
([[../.claude/rules/verify-what-arrived]]).

### 3. Whatever needs a human says so

Four kinds of repair, and the difference is who acts:

| Kind | Who does it | Example |
|---|---|---|
| `Auto` | the daemon, now | create the scheduled task, enable the unit |
| `sudo` | the person, in any terminal | install the udev rule |
| `Click` | the person, **in front of the machine** | macOS TCC |
| `Recipe` | the person, with the written recipe | run the deploy |

Mixing `sudo` with `Click` would promise automation that does not exist:
the first can be solved over SSH, the second requires human consent in a
system window.

## What lives where

| File | Role |
|---|---|
| `oxdesk/src/setup.rs` | catalog of the three systems + decision. **Pure**, with tests |
| `oxdesk/src/setup_common.rs` | measurements common to all three (config, peers, clipboard) |
| `oxdesk/src/setup_linux.rs` | binaries, unit, udev, Wayland, NVENC, tray |
| `oxdesk/src/setup_mac.rs` | launchd, identity, signing, TCC, Remote Login |
| `oxdesk/src/setup_win.rs` | `/IT` tasks, sshd, admin key, screen saver |
| `oxdesk/static/setup.html` | the page |
| `gui/check-setup.js` | smoke test of the page against the API |

The catalog is pure on purpose: the tests run on the development laptop
covering **all three** systems, not just the one being compiled. That is
what catches a dangling dependency or an out-of-order step before there
is a machine to test on.

## What the catalog guarantees (tested)

- unique ids on each system
- every dependency **exists** and **comes earlier** in the catalog —
  otherwise the step waits forever, or the screen shows "esperando"
  (waiting) above the thing being waited for
- every step says why it exists (`why`, with more than 40
  characters).
  A step without a reason becomes an automatic click
- `NotApplicable` neither counts nor blocks; `Error` blocks

## The dependency that matters most

On macOS, `screen-recording` and `accessibility`
**depend on** `signature`. This is not
fussiness about order: with an ad-hoc binary the TCC binding is by cdhash,
which changes with every build. Granting before signing wastes the grant,
and only someone at the machine can grant it again, by hand, in the UI.
This has already happened twice on the reference deployment — see
[[../.claude/rules/coordinate-spaces]] and `mac/create-identity.sh`.

## What the wizard does NOT do

- **It does not install binaries.** Compiling takes minutes and requires
  cargo; the path is `install.sh` or `scripts/deploy.sh`. The wizard says
  which one to run.
- **It does not run sudo.** The daemon is a user service. Asking for the
  root password in a local web page would teach a bad habit to save two
  commands.
- **It does not measure on its own in a loop.** The `peers` step opens
  SSH to each machine; a 10 s timer would turn a monthly question into a
  storm of connections. The page measures on open, after each repair,
  and when the person asks.

## Pending

- The macOS part is written and **has not been run**: the MacBook did
  not resolve via mDNS that day. What remains to check there is
  `--conferir` compiling, `launchctl print` as the measurement, and
  whether the TCC dialog appears when triggered by the daemon (which
  runs in the graphical session, so it should).
- `oxdesk/udev/70-oxdesk-hidraw.rules` went in alongside and was never
  installed on any machine: on the reference deployment the grant comes
  from the Solaar package's rule, which remains in `/usr/lib` even with
  Solaar removed.

## See also

- [[001-control-plane]] — the daemon that serves this page
- [[008-security]] — why the API only listens on 127.0.0.1
- [[../.claude/skills/configure-a-machine]] — the usage recipe
