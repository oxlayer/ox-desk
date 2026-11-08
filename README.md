# oxdesk

Drive several computers from one keyboard, one mouse and one panel, over
your own LAN. Switch machines, see their screens, share the clipboard and
move files — without a cloud account in the middle.

> **Status: alpha.** It runs every day on a three-machine deployment
> (Ubuntu, Windows, macOS) and that is where every number in
> [`specs/`](specs/) was measured. Treat anything outside that shape as
> untested.

## The gap it fills

Almost everything here exists somewhere in open source. One thing does
not: **deciding _how_ to switch computers** — move the pointer across the
LAN, or make the keyboard and mouse themselves jump Easy-Switch channel to
the other machine's receiver.

| Project | Switches the device channel | Knows the network |
|---|---|---|
| Solaar | yes (HID++ 0x1814) | no |
| Deskflow / Barrier | no | yes |
| GSConnect / KDE Connect | no | talks, but neither |

Nobody glues the three together. That is what this is; the rest is the
shell that makes it usable. Full argument in
[`specs/000-overview.md`](specs/000-overview.md).

## How it works

| Piece | What it does |
|---|---|
| [`oxdesk/`](oxdesk/) | The daemon and the panel: HTTP API on `127.0.0.1:8787`, machine switching, the setup wizard, clipboard and files |
| [`stream/`](stream/) | Remote screen and LAN KVM: capture, hardware encode, QUIC transport, input injection |
| [`hidpp/`](hidpp/) | HID++ 2.0 over raw HID — Easy-Switch channel switching. Useful on its own, and MIT for that reason |
| [`ota/`](ota/) | Downloads a published build, verifies it, signs it locally and swaps itself |
| [`gui/`](gui/) | Electron shell: window and tray, no logic |
| [`agent/`](agent/), [`winput/`](winput/) | The Windows side: CLI over SSH, and input injection |

Each machine runs the same install and is both host and client: it serves
its own screen and opens the others'. Trust is per machine, by key —
authorizing A to see B does not authorize B to see A.

Capture and encode use what each platform actually has: PipeWire with
Mutter's ScreenCast API on Linux, Desktop Duplication on Windows,
AVFoundation on macOS; NVENC where there is an NVIDIA GPU, VideoToolbox on
the Mac.

## Requirements, honestly

This is not a tool you can try on one laptop:

- **Two or more computers on the same LAN**, reachable by mDNS or a
  tailnet name. Never by IP — [`specs/007`](specs/007-device-routing.md)
  explains what that cost us.
- **Linux:** GNOME on Wayland. Capture goes through Mutter's private
  ScreenCast and RemoteDesktop APIs, so other compositors are not
  supported yet.
- **Windows:** OpenSSH Server, and the screen host runs from a scheduled
  task with an interactive token — a service lands in session 0, where
  there is no desktop to duplicate.
- **macOS:** Homebrew's `ffmpeg@7`, plus Screen Recording and
  Accessibility granted by hand. Those grants belong to the binary, so a
  rename makes macOS ask again.
- **Hardware channel switching** needs a Logitech Unifying receiver and
  devices that speak HID++ 2.0. Everything else works without it; only
  that feature does not.
- **NVENC** needs an NVIDIA GPU. Without one the screen path costs more
  CPU.

## Install

Build the workspace, install the daemon, then open the wizard — it checks
every prerequisite on the machine it runs on, explains what is missing and
fixes what it can:

```bash
cargo build --release --manifest-path oxdesk/Cargo.toml
install -m755 oxdesk/target/release/oxdesk ~/.local/bin/
oxdesk &                 # then open http://127.0.0.1:8787/setup
```

Repeat on each machine. [`docs/new-computer.md`](docs/new-computer.md) has
the order of operations and [`specs/011-onboarding.md`](specs/011-onboarding.md)
the design. Automatic updates are optional:
[`specs/015-ota.md`](specs/015-ota.md).

## Documentation

The specs are the real documentation: each says what was built, what was
measured, and what failed on the way.

- [`specs/000-overview.md`](specs/000-overview.md) — the gap and the shape
  of the product
- [`specs/005-input-switch.md`](specs/005-input-switch.md) — switching
  machines, HID++ and the LAN KVM
- [`specs/010-remote-desktop.md`](specs/010-remote-desktop.md) — the screen
  stack end to end
- [`specs/008-security.md`](specs/008-security.md) — keys, trust, and what
  never leaves a machine
- [`specs/018-open-source.md`](specs/018-open-source.md) — the plan for
  this repository being public, and what the pre-publish audit found

This file used to be a 901-line runbook about a Deskflow-based setup that
no longer exists, with IP addresses, a MAC address and three machines'
hostnames in it. Every lesson in it already lives in the specs above —
Deskflow alone is discussed in eight of them — so it was deleted rather
than anonymized. It is still there in the history the release tags reach:
`git show v0.2.8:README.md`.

## Contributing

Read [`CLAUDE.md`](CLAUDE.md) first: it indexes the rules that exist
because something already cost time. Three are not negotiable — nothing
shipped identifies a person or a machine, no credential lives in source,
and everything is in English.

A measurement beats an opinion. If you change behaviour, say what you ran
and what it printed.

## License

[AGPL-3.0-or-later](LICENSE), except [`hidpp/`](hidpp/), which is
[MIT](hidpp/LICENSE) so other projects can use the protocol work.

The macOS packages link Homebrew's FFmpeg, which is GPL-3.0-or-later.
[`THIRD-PARTY.md`](THIRD-PARTY.md) records that, and why it is what decides
the license of the whole distribution.
