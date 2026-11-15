# 006 — Screen sharing

## Decision: integrate, never reimplement

Building a RustDesk means capture (ScreenCast portal on Wayland, DXGI on
Windows), hardware H.264/AV1 encoding, latency-tuned transport, NAT
traversal, input injection and the crypto layer. Product-sized scope,
years of work — and three open projects have already solved it. `oxdesk`
orchestrates **lifecycle and credentials**, not protocol.

## What already exists on this LAN

### Ubuntu seen from Windows — already done, zero software

`gnome-remote-desktop` 50.2 **is installed and running** on the Ubuntu
machine with RDP **enabled** on 3389, its own TLS certificate and
credentials configured. Verified: listening, and Windows reaches it
(`Test-NetConnection … -Port 3389` → `True`).

From Windows: `mstsc` → `ubuntu-host.local`. Microsoft's client.

```bash
grdctl status          # state, port, fingerprint
grdctl rdp disable     # turn off
```

> **Warning:** it is a password-protected RDP server open to the whole LAN, on the
> main machine. `oxdesk` should **show this as a risk** in the panel, not
> as a silent feature.

### Windows seen from Ubuntu

Windows 10 **Pro**, so it has RDP — just turned off (3389 closed). Once
on, `remmina` or `freerdp3-x11` do the job. RDP takes over the local
session: it is no good for simultaneous use or for gaming.

### Gaming / low latency

**Sunshine** (host on Windows) + **Moonlight** (client on Ubuntu). Hardware
encoding, latency an order of magnitude below RDP or RustDesk.

### Outside the LAN

**Self-hosted RustDesk** (`hbbs`/`hbbr`). Self-hosted and under our own
control, without writing anything. On the LAN it does not even need a relay.

## What `oxdesk` does

| Does | Does not |
|---|---|
| show which routes exist and which are on | capture the screen |
| turn each server on/off | encode video |
| store credentials and remember fingerprints | transport frames |
| choose the route by use (see below) | inject remote input |

```
POST /api/hosts/{id}/screen   {"intent": "work" | "game" | "glance"}
```

- `work` → RDP (own session, crisp text)
- `game` → Sunshine/Moonlight (latency)
- `glance` → RustDesk (just look, without taking over the session)

Mapping intent to tool is the piece no existing tool provides. Choosing
between three clients with technical names is the friction today.

## Wayland: the caveat

Capture on Wayland goes through the `ScreenCast` portal, the same family
of problems we have already hit (`InputCapture`, `libportal` 0.9.1 too
old for the Deskflow clipboard). Before promising any route that
captures **the Ubuntu screen**, test it on 26.04 — do not trust the docs.

---

## Implemented (2026-11-03) — native to each OS, with the GUI dispatching

**Decided** against the alternative of a self-hosted RustDesk. RustDesk
had two real arguments in its favor — the same client on all three OSes,
and only **outbound** connections, which would matter
on a Mac with MDM — and lost because it is a large external dependency
and a relay to maintain, while the native route brings up no new daemon
anywhere.

Each machine already knows how to be controlled, through a different
protocol. There is no single one all three serve out of the box:

| host | protocol | server | status |
|---|---|---|---|
| Ubuntu | RDP | `gnome-remote-desktop` (**user** daemon) | already active, `*:3389`, TLS and credentials set |
| Windows | RDP | native `TermService` | turned on 2026-11-03, NLA mandatory, firewall LAN-only |
| macOS | VNC | Screen Sharing | to be enabled on the Mac |

### [`oxdesk/src/screen.rs`](../oxdesk/src/screen.rs) does not implement a protocol

It answers two questions — *is that host's screen reachable?* and *is
there a client here to talk to it?* — and, on request, launches the
client. Video, input and TLS belong to the programs that already exist.

That is why `GET /api/screen` is separate from `POST`: the UI needs to
know whether the button will work **before** the person clicks. A button
that opens nothing is worse than a disabled button with the reason
written next to it — and the reason is specific (*"no rdp client
installed here (looked for: …)"*, *"…:3389 did not answer"*), not an
"unavailable".

The per-platform client table lives in `sys::screen_clients`, in order of
preference. On Linux `gnome-connections` comes first because it speaks
**RDP and VNC**: one app covers Windows and the Mac, instead of two
clients with two UIs.

> `{u_flag}` exists in the template because freerdp wants `/u:name` as
> **one** argument, and when there is no user the argument has to
> *disappear* — passing an empty `/u:` makes it prompt for a user and
> hang without a terminal. Locked by
> `user_template_vanishes_when_there_is_no_user`.

### Windows was turned on with scope, not opened up

`fDenyTSConnections = 0` and the user-mode firewall rules enabled — but
with `-RemoteAddress 192.0.2.0/24` and only on the `Private,Domain`
profiles. The rule ships with `Any` in both, which would open RDP on
**any network** the laptop connected to. `UserAuthentication = 1` (NLA)
to authenticate before painting a login screen. The *shadow* rule
(watching someone else's session) stays off.

### Verified

- **Windows → Ubuntu: ready.** `mstsc.exe /v:ubuntu-host.local:3389`,
  target responding in 0.4 ms. Not triggered during verification: opening
  RDP against the user `gnome-remote-desktop` mirrors the session the
  person at the machine is using, so that click is left to them.
- **Ubuntu → Windows: 3389 already opens** from Ubuntu; only the client is
  missing, which comes in via
  [`scripts/enable-sshd.sh`](../scripts/enable-sshd.sh) together with
  `openssh-server`.
- **Mac**: among the three hosts in the config with `expect = "any"`, it
  lights up on its own once Screen Sharing is enabled.

### Missing

- A VNC client on Windows (nothing ships out of the box) to reach the Mac.
- An RDP client on the Mac — `brew install --cask windows-app`.
- A shortcut in the Electron tray, today only in the panel.
