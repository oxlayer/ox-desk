# 000 — Overview

> Audience: contributors, and the coding agents that work on this repository.

## The gap that justifies the project

Of the capabilities in scope — SSH, file browser, KDE Connect, switching
PCs by edge or shortcut, clipboard, screen sharing — **almost everything
already exists and works well in open source**. Reimplementing it would
mean spending months to end up with something worse.

One thing does not exist. This one:

> Deciding **how** to switch computers: move the cursor over the LAN, or
> make the mouse/keyboard itself jump Easy-Switch channel to the other
> machine's physical receiver.

That is exactly what **Logitech Flow** does, it was the original goal of
the project, and it is the only piece no open project solves:

- **Solaar** knows how to switch the device channel (`change-host`, HID++
  0x1814) but knows nothing about the network.
- **Deskflow** knows how to move the cursor over the network but does not
  touch HID++.
- **GSConnect / KDE Connect** know how to talk to the other machine but
  do neither.

Nobody glues the three together. **That is the product.** The rest is the
shell that makes it usable and that reuses what already exists.

## What we build and what we integrate

The criterion: we build **orchestration, state and UI**. We integrate
**transport, codec and protocol** — layers where a mature project has
already solved problems that would take years.

| Capability | Decision | Why |
|---|---|---|
| Input routing (LAN ↔ physical receiver) | **build** | does not exist; it is the core of the project |
| Control plane + API + state | **build** | it is the glue; nobody does it |
| Configuration GUI (Electron) | **build** | we already have the shell and the panel |
| SSH orchestration / machine bootstrap | **build** | specific to the machines oxdesk drives |
| File browser | **build** (thin) | it is UI over SFTP, which OpenSSH already provides |
| Clipboard | **build** (thin) | it is a text relay; the hard part is policy, not protocol |
| Moving the cursor between screens | **integrate** Deskflow / lan-mouse | protocol + input injection via portal; mature |
| Screen sharing / remote control | **integrate** RustDesk (self-hosted) or Sunshine+Moonlight | PipeWire/DXGI capture, H.264/AV1 encode, NAT, latency. Product-sized scope. |
| LAN discovery and TLS pairing | **integrate** the KDE Connect model | tested protocol, and the reference deployment was already paired |

Reimplementing a RustDesk would mean: screen capture (ScreenCast portal
on Wayland, DXGI on Windows), hardware encoding, latency-tuned transport,
NAT traversal, input injection, crypto and auth. It is not a weekend
project, and RustDesk is **self-hostable** — our own `hbbs`/`hbbr`, or a
direct LAN connection, with no third-party server. Self-owned and secure
without writing any of it.

## Architecture

Symmetric: **every node runs the same daemon and none is central.** The
machine in charge is the one where the person is. See
[`001`](001-control-plane.md#symmetric-every-node-is-a-peer-none-is-central).

```
   ubuntu-host              windows-host            mac-host (channel 3)
  ┌──────────────┐        ┌──────────────┐        ┌──────────────┐
  │ GUI          │        │ GUI          │        │ GUI          │
  │  ↕           │        │  ↕           │        │  ↕           │
  │ oxdesk        │◀─SSH──▶│ oxdesk        │◀─SSH──▶│ oxdesk        │
  │  ├ input     │        │  ├ input     │        │  ├ input     │
  │  ├ hidpp     │        │  ├ hidpp     │        │  ├ hidpp     │
  │  └ adapters  │        │  └ adapters  │        │  └ adapters  │
  └──────┬───────┘        └──────┬───────┘        └──────┬───────┘
         │ channel 1             │ channel 2             │ channel 3
         └───────────── MX Anywhere 3 + MX Keys ──────────┘
                    (exactly one holder at a time)
```

Requesting focus is forwarding: whoever asks talks to the **current
holder**, because only it reaches the device over radio.

## Language: Rust

The control plane is **Rust** ([`oxdesk/`](../oxdesk/)), a decision made on
2026-11-03 and already implemented in phase 1.

The prototype was Python stdlib, and it worked. The migration happened
before complexity grew — which is the cheap moment to do it, not
afterwards. What was gained, measured:

| | Python | Rust |
|---|---|---|
| `/healthz` | 0.6 ms | **0.19 ms** |
| binary | — | 929 KB, no runtime |
| invalid config | runtime error, on the request | **rejected at boot**, with exit 1 |
| unknown field in PATCH | would need manual validation | rejected by the type, 400 |

The real gain is not latency (I/O dominates: `/api/status` takes 2 s in
both versions, because of SSH). It is the **type as contract**: `Expect`
is an enum, not a string; `Config` does not deserialize if the TOML is
wrong; and the compiler rejects the unhandled case. In a daemon that will
trigger HID++ channel switches and can leave you without a keyboard
([`005`](005-input-switch.md#there-is-no-rollback-on-this-side)), that is worth
more than microseconds.

Dependencies, deliberately few: `tiny_http` (blocking — two clients do
not justify async), `serde`, `serde_json`, `toml`. No date crate:
`date +%z` resolves the local offset.

The **Windows agent** will also be Rust, for the same reason HID++
0x1814 will be ours: a single binary, no runtime to install on the other
side.

## Phases

| Phase | Deliverable | Status |
|---|---|---|
| **0** | Manual setup working, documented | Done — see [README](../README.md) |
| **—** | Deskflow and Solaar **removed** from both sides | Done — the switch is physical; neither is needed anymore |
| **1** | `oxdesk` serving status + editable config | Done — Rust, `PATCH /api/config` validated |
| **2** | SSH + automatic bootstrap of both machines | [`002`](002-ssh.md) |
| **3** | Input routing: LAN × physical receiver | In progress — `GET /api/devices` and `POST /api/focus` (dry-run) ready; edge detection and the Windows agent are missing |
| **4** | Clipboard and file browser | [`003`](003-files.md), [`004`](004-clipboard.md) |
| **5** | Screen sharing (integration) | [`006`](006-screen-share.md) |

Phase 3 comes before 4 and 5 on purpose: it is the only one nobody else
does.

## Non-goals

- Internet, cloud, accounts, telemetry. LAN only.
- Supporting hardware beyond the reference deployment's. Generalize later,
  when there is a need.
- Reimplementing a video codec, KVM protocol or discovery stack.
- Mobile. GSConnect already handles phones.

## Read also

- [`001-control-plane.md`](001-control-plane.md) — the daemon and the API
- [`005-input-switch.md`](005-input-switch.md) — the core of the project
- [`008-security.md`](008-security.md) — identity and trust model
- [`../README.md`](../README.md) — what is already running today
