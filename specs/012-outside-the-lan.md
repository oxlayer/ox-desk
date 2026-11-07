# 012 — Reaching the machines outside the LAN

> Status: all three machines **inside** the tailnet. The Mac joined via
> brew's `tailscaled`, not via the app (see "The Mac"). `--bind` and
> `--mbps` are ready and measured.

## What changes relative to spec 008

[Spec 008](008-security.md) says "everything on the LAN, no account, no
cloud". This spec opens a narrow exception and states the price.

What does **not** change: `logid` keeps listening only on `127.0.0.1`. The
panel from outside arrives over an SSH tunnel (`ssh -L 8787:127.0.0.1:8787 …`),
which is literally what 008 already said to do. No control
port starts listening on the network.

What changes: the three machines join an overlay, and the MacBook can
reach them from anywhere.

## Why an overlay and not an HTTP tunnel

The screen transport is QUIC, which is **UDP** ([spec 010](010-remote-desktop.md)).
That eliminates half the options before starting:

| Tool | Carries our UDP? | Path |
|---|---|---|
| Cloudflare Tunnel (`cloudflared`) | ❌ TCP/HTTP only | — |
| Cloudflare Mesh (formerly WARP Connector) | ✅ TCP/UDP/ICMP | **relay through the Cloudflare edge** |
| Tailscale | ✅ | direct between peers; relay (DERP) only when NAT prevents it |
| Plain WireGuard | ✅ | direct, but **no NAT traversal**: requires a reachable endpoint |

**Tailscale** was chosen. What decided it was not ease of use: it was the
path. Every screen frame crosses the network 60 times per second, and
Mesh puts the Cloudflare edge in the middle of all of them. Tailscale tries
direct first.

Plain WireGuard would be more faithful to "no cloud", but it would need a
public endpoint — port-forwarding with a dynamic IP, or a VPS. More pieces
to break, and Tailscale's coordination only sees connection
metadata: the traffic is end-to-end WireGuard between the machines.

### Accepted risk

A third-party account becomes part of the path: if Tailscale's coordinator
is down, a new machine cannot join and an expired key does not
renew. Sessions already established keep going. It is the same kind of
dependency that 008 avoided until now, and it comes in as a conscious risk,
not as an oversight.

## The exposure this creates, and what was done

The screen channel **encrypts and does not authenticate**. The client accepts any
certificate (`AceitaQualquer`, in `logistream/src/net.rs`, marked as
temporary since it was written) and the host never asked the client for anything.
Whoever reaches 9777 watches; whoever reaches 9778 types.

While the three machines stayed at home, "whoever reaches" was the home
LAN. **The MacBook leaving changes that**: on `0.0.0.0`, on a hotel Wi-Fi,
it serves screen and keyboard to the whole hotel network.

### Done: `--bind`

```
logistream-host [porta] [tela] [--bind todas|overlay|<endereço>] [--mbps N]
```

- `todas` (all, the default) — `0.0.0.0`, exactly as always. No machine
  changes behavior without someone asking.
- `overlay` — only the overlay address.
- `<endereço>` (address) — by hand.

Two decisions the code enforces, each with a test that breaks if
someone "improves" it:

**`overlay` fails when it does not find the overlay.** It does not fall back to `0.0.0.0`.
The silent fallback would be comfortable and would be the bug: the person asked for
protection, Tailscale did not come up, and the host would open to the hotel without
anyone knowing.

**Range + interface name, never just the range.** 100.64.0.0/10 is
CGNAT (RFC 6598) and Tailscale uses it — but it is real CGNAT
too. A phone sharing its internet hands out addresses in that range
to whoever connects, so the MacBook on a hotspot (precisely the test
scenario) would have an `en0` in 100.x, and the naive criterion would listen on the
phone's network thinking it was the overlay.

### Pending: key-based pairing

The right thing is what 008 already describes for the other three channels: trust on
first use with human confirmation. The screen channel is the only one that violates the
project's own rule. `--bind` closes the door; it does not change the lock.

## Bandwidth: `--mbps`

The bitrate target scales with the area (~8 Mbps at 1080p, ~15 at 1440p,
~28 on Retina). The formula was **duplicated** — `nvenc.rs` and, by hand,
`host_mac.rs` — and now lives in `quality.rs`, with a test.

**Measured:** the home upload delivers ~136 Mbps (25 MB in 1.46 s against
speed.cloudflare.com). That is, **the bottleneck is not on this end**: 15 Mbps
goes out effortlessly. What squeezes is the other side — hotel Wi-Fi usually
gives 10–50 Mbps of download, variable, with queueing.

That is why the cap exists and why it is chosen for the remote
session, not stored on the machine.

### What the cap is not

It is not congestion control. Chosen once, it does not move. If
the link gets worse mid-session, nothing reacts.

The next step is to close the loop, and the signal already exists for free:
`quinn` measures RTT, loss and congestion events on the path
(`Connection::stats()`), and the host already counts `recusados` when
congestion control refuses a datagram. Nothing consumes either
of them. Feeding that back into the encoder is, in one sentence, what Parsec's BUD
does differently from ours.

## mDNS does not die, and this was measured

A concern raised and **discarded by measurement**: on this machine `.local`
names do not go through systemd-resolved.

```
hosts: files mdns4_minimal [NOTFOUND=return] dns
```

Avahi answers through NSS **before** `dns`, so any resolver
Tailscale configures is not even consulted for `.local`.
`resolvectl` confirms: mDNS is off on all links, and
`getent hosts windows-host.local` resolves anyway.

If it ever breaks: `sudo tailscale set --accept-dns=false`.

## Recipe

```bash
# Ubuntu (needs sudo -- the daemon does not install packages on its own)
sudo bash scripts/instalar-tailscale.sh
sudo tailscale up

# Windows (done: MSI with TS_UNATTENDEDMODE=always)
#   `always` keeps the tunnel up WITHOUT a logged-in user -- the machine
#   stays reachable at the lock screen, which is the state it will be
#   in when I want to reach it from outside.

# check, on BOTH sides -- "direct" is a property of the pair
bash scripts/conferir-overlay.sh
```

## The test that counts

Measuring "outside the LAN" from inside the house proves nothing. The only
honest test is **tethering the MacBook to a phone hotspot** (turning off the
home Wi-Fi) and opening a real screen. What matters to measure there:

- does `tailscale ping` say `direct` or `via DERP`? Relay = CGNAT on both
  sides; it works, but the screen feels it.
- fps and dropped frames in `logistream-sink`.
- pointer latency, which is what the person notices first.

## The Mac — via brew, never via the app

The plan was `tailscale-app`, the cask with a system network extension.
On 2026-11-07 it took down the Mac's internet, and it was removed.

Without a completed login, the extension stayed at "Connecting…" with **Connect
on demand** turned on. Measured in the log (macOS 14.0, app 1.104.5):

```
Failed to current profile: notFound
setGUIVisible=true failed: noSessionID
Tunnel did not connect ...  stopTunnel: "connectionFailed"
```

It gave up after 5 minutes and on demand turned it back on. Meanwhile the Mac
had no internet, and the login needs internet to open. The way out
was to delete the configuration in Settings → VPN. Clicking "Allow" repeated everything.

The `/usr/local/bin/tailscale` that the app installs is a script that **opens the
app**, so even a `tailscale status` through it triggers the VPN request.
The measurement changed what was being measured.

The path that worked is brew's `tailscaled` as root, with `utun`:

```bash
brew install tailscale
sudo bash logid/setup/instalar-tailscale-mac.sh
```

The wizard's `tailscale` step does this on its own: the script ships inside the
binary, and the button opens Terminal with it.

It does not go through the extension, it only adds the route for `100.64.0.0/10`, and
the default route stays where it was. Tailnet names work via
`/etc/resolver`, only for the tailnet domain: general DNS does not change. The
script undoes the step if the internet fails. The wizard measures this in the
`tailscale` step, and counts the installed app as a fault.

Measured on the Mac: `100.64/10` on `utun15`, default route on `en0`, SSH to the
other two by short name.

Userspace mode (`--tun=userspace-networking`) also works, without
root, but Mac apps only reach the tailnet through a SOCKS proxy.

**The tailnet name goes into the config, in place of `.local`.** Outside the
LAN mDNS does not resolve, and the panel marks the machines as dead. It is a
name, so the rule of not writing IPs still holds.

## What is missing

- Mac: `--bind overlay` on the screen host.
- Our own protocol in place of Tailscale, to get out of the dependency
  on the account and the coordinator (the "Accepted risk" above).
- Bitrate adaptation via `quinn::Connection::stats()`.
- Pinned-key pairing on the screen channel.
- The hotspot test (above), which is the only one that counts.

## See also

- [[008-security]] — the trust model this spec carefully opens
- [[010-remote-desktop]] — why QUIC, and why that constrains the overlay
- [[../.claude/skills/configure-a-machine]] — the wizard, which already measures SSH between peers
