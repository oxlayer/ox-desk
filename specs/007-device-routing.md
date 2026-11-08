# 007 — Discovery, names and routing

## Inviolable rule: never write an IP

On 2026-11-03 Windows moved from `192.168.1.43` to `192.168.1.42` (same
MAC `00:00:5e:00:53:01`). It broke all at once: Deskflow (`No route to
host` in a loop), SSH, and the panel. **GSConnect survived** — it
discovers by broadcast.

The lesson: what discovers by name heals itself; what is pinned to an IP
breaks.

## mDNS

```
/etc/nsswitch.conf:  hosts: files mdns4_minimal [NOTFOUND=return] dns
avahi-daemon:        active
libnss-mdns:         0.15.1-5
```

`getent hosts windows-host.local` → `192.168.1.42`, stable. All configs
use the name: `Deskflow.conf`, `~/.ssh/config` and
`~/.config/oxdesk/config.toml`.

Quirks paid for:

- **Deskflow does not accept a comma-separated list** in `remoteHost`,
  despite the GUI tooltip promising it. `FATAL: No server addresses
  configured`. A single host only.
- **`ssh-keyscan <name>.local` fails** with `Temporary failure in name
  resolution` even with `getent` working — `mdns4_minimal` +
  `[NOTFOUND=return]` on an AF_UNSPEC query. `ssh` itself resolves.
- **`known_hosts` is per name.** Switching IP→name gives `Host key
  verification failed`. The right fix is copying the *same* trusted key
  to the new name and checking the fingerprint — never a blind
  `accept-new`.

## What `oxdesk` must do

1. **Always resolve on the spot.** Never cache an IP in config; at most
   cache it at runtime, with a short TTL.
2. **Warn when the IP changes.** Even when working by name, record the
   event: a wandering IP is a symptom of a missing DHCP reservation.
3. **Check prerequisites at startup** and expose them in the API:
   `avahi-daemon` active, `mdns4_minimal` in nsswitch, `hidraw` ACL.

## Third peer: MacBook on channel 3

Read from the hardware (the name comes from the keyboard, which has
`HOSTS INFO {1815}`):

```
solaar config 2 change-host
  -> [ 1:ubuntu-host, 2:windows-host, 3:ASSET-TAG-0001 ]
```

Channel 3 is a **MacBook**. The name `ASSET-TAG-0001` is a corporate
asset tag, not the hostname mDNS will announce — when integrating, check
its real `.local`.

It is **not** in `[[hosts]]` yet: a peer without a host block does not
enter routing, and that is better until the daemon is running there.
What it needs, when its turn comes:

| Piece | Situation on macOS |
|---|---|
| sshd | ships with the system, just enable it in Settings |
| mDNS | native (Bonjour) — `.local` works out of the box |
| HID++ | needs a `oxdesk` build for arm64/x86_64 Darwin |
| Easy-Switch channel | 3, already paired and confirmed |

The symmetric model ([`001`](001-control-plane.md#symmetric-every-node-is-a-peer-none-is-central))
was designed precisely for this: the third peer is not a special case,
it is just one more node with the same binary.

## Pending item outside our reach

DHCP reservation on the router for MAC `00:00:5e:00:53:01`. mDNS depends
on avahi and multicast; the reservation is the redundancy. It cannot be
automated — it is an action on the router.

## Device identity (Logitech)

See [`005`](005-input-switch.md). Mapping on this machine:

| hidraw | device | receiver slot |
|---|---|---|
| `hidraw0` | `046D:C52B` Unifying receiver | — |
| `hidraw2` | `046D:4090` MX Keys | 2 |
| `hidraw3` | `046D:408A` MX Anywhere 3 | 1 |

Both are paired here **and** on the Windows receiver, on different
Easy-Switch channels. That is what makes the physical route possible.
