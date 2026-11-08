# The three machines

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| name | `ubuntu-host` | `windows-host` | `mac-host` |
| address | local | `windows-host.local` | `mac-host.local` |
| OS | Ubuntu 26.04, GNOME 50.1, Wayland | Windows 10 Pro | macOS 14 (Apple Silicon) |
| user | `user` | `user` | `user` |
| ssh | — | `ssh windows-host` | `ssh mac` |

**Never write an IP in config.** The Windows IP changed once and broke
everything at once. Use mDNS (`avahi` + `libnss-mdns`), or the tailnet
name (`ubuntu-host`, `windows-host`) on the machine that leaves the
house: see [`specs/012`](../specs/012-outside-the-lan.md).

## Paths that differ

This is where time gets lost. None of it follows a single pattern.

| What | Ubuntu | Windows | macOS |
|---|---|---|---|
| config | `~/.config/oxdesk/config.toml` | `%APPDATA%\oxdesk\config.toml` | `~/.config/oxdesk/config.toml` |
| binaries | `~/.local/bin/` | `%LOCALAPPDATA%\oxdesk\` | `~/.local/bin/` |
| agent process | `oxdesk` | `oxdesk.exe` | `oxdesk` |
| restart | `systemctl --user restart oxdesk.service` | `taskkill /F /IM oxdesk.exe` + `schtasks /run /tn oxdesk` | `launchctl kickstart -k gui/$(id -u)/app.oxdesk.daemon` |
| screen host log | `~/.local/state/oxdesk-host.log` | `%LOCALAPPDATA%\oxdesk\oxdesk-host.log` | same as Ubuntu |

Note that the scheduled **task** on Windows is called `oxdesk` while the
**executable** is `oxdesk.exe` — and the screen host is `oxdesk-host`
in both. Always check with
`schtasks /query /fo table | findstr /i oxdesk`.

## Ports

| Port | Service |
|---|---|
| 8787 | `oxdesk` API + panel (each machine serves its own) |
| 9777 | `oxdesk-stream` screen host (QUIC: video by datagram, input by stream) |
| 22 | SSH (Windows and Mac) |
| 1716 | ~~KDE Connect~~ — removed on 2026-11-04, replaced by the native clipboard |
| 5900 | Mac VNC — stays while the Mac host doesn't produce frames |

RDP (3389) was removed on 2026-11-04: the four screen paths are ours.

## The Windows interactive session

SSH on Windows lands in **session 0**, where there is no desktop.
Nothing that needs a screen or `SendInput` works over plain SSH — not
duplicating the screen, not injecting input, not turning off the
monitor.

What works: a scheduled task with `/IT`, which runs in the interactive
session. That is how `oxdesk` and `oxdesk-host` start.

## See also

- [[screen-paths]]
- [[../.claude/rules/config-per-machine]]
- [[../.claude/rules/restart-the-agent]]

## How input switches machines

`input.mode` in the config:

| Mode | Mechanism | What it needs |
|---|---|---|
| `lan` (default) | captures here (`EVIOCGRAB`) and sends over the remote screen's input stream | ACL on `/dev/input` ([[../.claude/skills/grant-input-access]]) and the screen host up on the destination |
| `hidpp` | tells the device to switch Easy-Switch channel (0x1814) | a receiver paired **on the destination machine** |

The default became `lan` when we were left with a single dongle: in
`hidpp`, "send to windows-host" made the keyboard look for a receiver that
doesn't exist there, not find it, and come back.

A difference that matters in the UI: in `hidpp` the device **disappears**
from our receiver when it switches, so "some device offline" was the
signal for "it's away". In `lan` it never leaves here. What answers
"where is input going" is `GET /api/kvm`.

Capture only exists on Linux for now. Process log:
`~/.local/state/oxdesk-kvm.log`.
