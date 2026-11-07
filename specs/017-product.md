# 017 — From personal infra to product

> Status: decided on 2026-11-07. Phase A of pairing live since 2026-11-07 (#11,
> build #166 on all three machines).

## The decision

ox-desk stops being just the infra for three computers and becomes a
remote-access product, in the same category as Parsec and AnyDesk:

- **open source**, and anyone can host their own server;
- **or use our servers**, which is the paid version;
- **basic telemetry, which can be turned off**, to measure what decides the
  product: how many connections go direct and how many fall back to the relay.

This changes two old premises, and this spec is the record of those changes:

| Before | Now |
|---|---|
| "No telemetry, no cloud" (`CLAUDE.md`, spec 008) | Minimal telemetry, on by default, which can be turned off. The server is optional and can be hosted by anyone |
| Trust coming from the LAN and the overlay (spec 012) | Trust coming from **each machine's key**. The network, including our server, never needs to be trusted |

## Two rules that are not lost

**LAN-only keeps working.** With no internet, no server and no
telemetry, machines on the same network find each other via mDNS and connect directly,
with the same key-based pairing. The rendezvous server only comes in when
direct LAN does not exist. No local feature may come to depend
on it: losing the server takes away access from outside, and never from inside the house.

**Every computer is host and client.** It is a single install, with a single
identity, that serves its own screen and opens others'. The list of
who may connect (`autorizados`) belongs to each machine, and authorizing A to see B
does not authorize B to see A.

## The name: oxdesk

"logi" and "logistream" come from Logitech, from the time when the project was about switching
keyboard and mouse between PCs. In the product, Logitech is **one feature** (Easy-Switch
channel switching for whoever has their keyboard and mouse and several PCs), and the
name becomes **oxdesk**.

| Today | After |
|---|---|
| `logid` | `oxdesk` (daemon and CLI) |
| `logistream-host`, `-view`, `-ponte`, `-kvm` | `oxdesk-host`, `-view`, `-ponte`, `-kvm` |
| `logi-ota` | `oxdesk-ota` |
| `~/.config/logi`, `%APPDATA%\logi` | `~/.config/oxdesk`, `%APPDATA%\oxdesk` |
| `%LOCALAPPDATA%\logid` | `%LOCALAPPDATA%\oxdesk` |
| `ai.logi.*` (launchd), task `logid`, `logid.service` | `app.oxdesk.*`, task `oxdesk`, `oxdesk.service` |
| `LOGI_*` (`LOGI_PAR`, `LOGI_PONTE_TOKEN`...) | `OXDESK_*` in English (`OXDESK_PIN`, `OXDESK_BRIDGE_TOKEN`...), accepting the old one for a while |
| crate `hidpp` | stays: it is Logitech's protocol, and the name is theirs |

It is a two-sided change, and it has three traps measured beforehand:
- **The installed OTA only knows the old names.** A transition version
  has to know how to install the new one and swap its own supervisor. Without it, the
  machine that updates itself (Windows, Mac) stops at the old version.
- **On the Mac, the Screen Recording and Accessibility permissions belong to the
  binary.** With the new name, the person grants them again, by hand.
- **On Windows, the Defender exclusion is for the path** `%LOCALAPPDATA%\logid`.
  The new folder needs the exclusion before the binary arrives, or it is
  deleted on write (spec 008).

**And everything that is not on-screen text is in English:** file name, script,
binary, module, identifier, environment variable, config key, launchd
and task label. Portuguese only in what the person reads on screen, in the logs
they read and in the specs. It goes into the same PR as the rename: `assinar.sh` →
`sign.sh`, `construir-binarios.sh` → `build-binaries.sh`, `ponte.rs` →
`bridge.rs`, `qualidade.rs` → `quality.rs`, `setup_comum.rs` →
`setup_common.rs`, `instalar-tela-virtual.ps1` → `install-virtual-display.ps1`,
and so on. Whatever is called from outside (deploy, OTA, tasks, launchd)
changes along with it, and searching for the old name has to return zero at the end.

The config and the identity migrate on first start: if the new folder does not
exist and the old one does, it is moved, and the log says so.

It goes in its own PR, after pairing phase A is live.

## The order

1. **Key-based pairing.** It applies in every scenario, including today's
   Tailscale.
2. **Path telemetry**, to measure how much falls back to relay: at home, on 4G
   and outside.
3. **A rendezvous and relay binary** (`logi-encontro`), which we host
   and which other people can host too.
4. **Cloudflare only as a hosting option**, and never as a dependency.
5. **Turn off Tailscale**, only after our own path has run on its own for
   a while.

## 1. Key-based pairing

### What exists today

- The host generates a new certificate on every start.
- The client accepts any certificate (`AceitaQualquer`, in `net.rs`).
- The server's TLS signature is not checked either.
- The host does not ask who the client is.

In practice, the channel encrypts and does not authenticate. Whoever reaches port 9777 sees the
screen, and whoever reaches 9778 types into it.

### The design

- **Every install has an identity:** a certificate with the key
  (ECDSA P-256), generated once and stored with permission `600`. The fingerprint is the
  SHA-256 of the certificate.
- **The host presents this identity**, not an ephemeral certificate.
- **The client checks the host's fingerprint** when it receives it (`LOGI_PAR`). The
  TLS signature starts being verified for real. Without this check,
  anyone could present the host's certificate, which is public, without having
  the key.
- **The client also presents its identity.** The host checks this
  identity against the list of who it authorizes
  (`<config>/identidade/autorizados`, one fingerprint per line).

### Where the fingerprint comes from

- **Today, between our machines:** via SSH, which is already authenticated by
  `known_hosts`. `logid` reads the fingerprint on the other side and pins it. It is an
  authenticated channel, not trust on first use.
- **In the product:** via the pairing code. The person reads a short code on
  both screens and confirms, as in spec 008: `logid` may show the codes
  side by side, but may never accept on its own.

### The phases (the server goes first)

The host is the one that accepts, so it goes up first. Refusal is only turned on after a
period of observation, as the `observe` → `enforce` lesson says.

| Phase | Host | Client |
|---|---|---|
| **A, observe** | Presents the persistent identity. Asks for the client certificate, without requiring it, and logs `cliente <digital> autorizado/desconhecido/sem certificado` | Presents the identity. Checks the host fingerprint when there is `LOGI_PAR` and refuses if it does not match. Without `LOGI_PAR`, it carries on as today and warns in the log |
| **B, pin** | Same as phase A | `logid` passes the fingerprint over SSH. Without a fingerprint, the panel does not open the session |
| **C, require** | Refuses clients without a certificate and clients not on the list | Same as phase B |

The switch to phase C only happens after the phase A log shows zero
`desconhecido` and zero `sem certificado` in the legitimate connections of the three
machines.

## 2. Telemetry

### What is sent

One event per session, at its end:

| Field | Example |
|---|---|
| version, host and client OS | `0.3.0`, `windows`, `macos` |
| path | `lan`, `overlay-direto`, `overlay-relay`, `nosso-direto`, `nosso-relay` |
| NAT type, when measured | `cone`, `simetrico` |
| RTT p50 and p95, fps, average Mbps, dropped frames | |
| duration, in rounded minutes | |
| end reason | `fechou`, `caiu`, `recusado` |

### What is never sent

- IP, machine name, user name, file path or the content of the
  screen, keyboard or clipboard;
- the identity fingerprint. The install id is a random value, separate from it,
  and changes when telemetry is turned off and on again.

### How to turn it off

- `telemetria = false` in `config.toml`, or the switch in the panel;
- `LOGI_TELEMETRIA=0` in the environment, which overrides the configuration;
- whoever hosts their own server chooses where the telemetry goes, or
  turns it off for all clients of that server.

Before the first send, the panel shows what is sent, and keeps the last
event visible for the person to check.

### While there is no server

The event is written to a local file (`<estado>/telemetria.jsonl`). That already
answers the path question with Tailscale: `tailscale status --json`
says whether the peer is direct or on relay (DERP).

## 3. Rendezvous and relay

A single binary, `logi-encontro`, and a Docker image:

- **Rendezvous:** each machine keeps a connection to it and announces the
  addresses it sees (the local one and the public one, measured like STUN). When someone
  wants to connect, the server introduces them to each other, and both try the
  direct connection (*hole punching*) over the same UDP socket as QUIC.
- **Relay:** if direct does not come through in ~2 s, traffic goes through the server. Since
  it is QUIC encrypted end to end with the machines' identity, the relay
  forwards bytes it cannot read.
- The server knows **who talks to whom and when**. It never sees the screen, the keyboard
  or the files.

Design references: RustDesk's hbbs/hbbr and Tailscale's DERP.

### Where to host and how much it costs

The cost is relay bandwidth. Signaling is negligible.

| Option | Bandwidth | Note |
|---|---|---|
| VPS in São Paulo | a few TB included | Cheap, but we operate it |
| Cloudflare (Realtime TURN) | ~1 TB free/month, then ~US$ 0.05/GB | No operations; price to be checked before deciding |
| AWS sa-east-1 | ~US$ 0.15/GB | Expensive for video |
| Hetzner, DigitalOcean | cheap bandwidth | No region in Brazil: adds latency |

The binary must not depend on Cloudflare. It comes in, if at all, as one more
place to run the relay.

## Plans

The structure follows Parsec's, which is the market reference: free for
personal use, and the paid tier sells what costs us money (relay, team management,
auditing). Parsec's prices on 2026-11-07, per user per month: US$ 9.99
(individual), US$ 35 (teams) and US$ 45 (enterprise, annual only).

| Plan | What it has |
|---|---|
| **Free** | The whole core, as in Parsec: 60 fps with low latency, keyboard, mouse and controller, end-to-end encrypted connection, host on Windows, macOS and Linux, share the screen by link. Plus: LAN, direct connection via our rendezvous, and unlimited self-hosted server. Our relay with a bandwidth cap |
| **Individual** | Unlimited relay, multiple monitors, virtual monitor, 4:4:4 color, privacy mode (physical screen blanked) |
| **Teams** | Team computers in one place, per-person and per-machine permissions, single sign-on (SAML/OIDC), 7-day access log |
| **Enterprise** | Dedicated relay or in the customer's region, full access log, administrator roles, API for automation |

Of the core, we do not have yet: controller (gamepad) and share by link,
which is a single-use, time-limited invite that gives access to a machine without
permanent pairing.

What stays free is what costs us nothing. Hosting your own server is
always free, and it is what keeps the project truly open source.

### What else Parsec does, and how it works here

Read at parsec.app/pricing, on 2026-11-07:

| In Parsec | Here |
|---|---|
| Free only for personal use; work requires a license | The code is MIT and cannot carry that restriction. It applies only to **our servers**, through the terms of use. Whoever hosts their own server uses it for whatever they want |
| 14-day trial of Teams and Enterprise | Same |
| Monthly, or annual with a discount; Enterprise annual only; no volume discount | Same, to start with |
| Collaboration by link: the other person watches or takes control | The free link invite, with two modes: view only, or view and control |
| Guest access (Teams): credits bought separately, up to 5 days | Time-limited invite without permanent pairing. Credits sold separately, in Teams |
| Team computers, assigned to a person or a group | That is the "Teams" in the table above |
| In Teams, members only connect to members | Authorized list coming from the team, not from each machine |
| Block machines on the company network from serving their screen | Team policy that the host obeys |
| Relay inside the customer's firewall (Enterprise) | `logi-encontro` hosted by the customer, which is the same open source binary |

## What to reuse from Parsec's open repositories (read on 2026-11-07)

Cloned and only read, without compiling or running:

| Repository | License | For us |
|---|---|---|
| `libmatoya` (C, active) | MIT: can be ported, with the copyright notice | Controller in `src/hid/` (PS4, PS5, Switch, Xbox, rumble); client-side audio (`windows/audio.c`, `resample.c`); RGBA cursor drawn on the client, Alt+Tab and Windows-key hook, `keyboard.lock` and pointer lock (`windows/appw.c`, `unix/web/matoya.js`) |
| `go-stun` (Go, 2018) | MIT | Binding Response format with `XOR-MAPPED-ADDRESS`, for `logi-encontro`. STUN and QUIC on the same socket are separated by the first byte (RFC 7983) |
| `Parsec-Cloud-Preparation-Tool` (PowerShell) | no license: reference only | List of VM tweaks: auto-logon, remove the lock screen, disable the basic display adapter, install the virtual monitor driver |
| `Cloud-GPU-Updater` (PowerShell) | no license: reference only | A cloud GPU in TCC mode has no video output; switching to WDDM is a `doctor` item for hosts in a VM |

None of them covers the host side we are missing: audio capture, lock screen and
UAC, Ctrl+Alt+Del and the SYSTEM service. The Preparation Tool's "MouseKeys as cursor"
was measured here and does not work (`.claude/rules/cursor-without-mouse.md`).

## What this spec does not decide

- The price in reais, and what goes into the free relay cap.
- User account and machine list in the cloud: it starts without accounts, only with
  id and pairing.
- The endpoint that receives telemetry (comes with `logi-encontro`).

## See also

- [[008-security]] — the trust model this spec changes
- [[012-outside-the-lan]] — why an overlay, and what is missing
- [[016-session-and-visuals]] — the session in a tab, which starts carrying the fingerprint
