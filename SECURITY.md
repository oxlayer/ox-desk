# Security

## Reporting a vulnerability

Use GitHub's **private vulnerability reporting** on this repository
(Security → Report a vulnerability). It is private, it needs no e-mail
address from either side, and it keeps the disclosure in one place.

Please do not open a public issue for a vulnerability.

No personal e-mail address is published here on purpose: a security
contact has to be a role, not a person, or it rots the first time someone
changes jobs — and this repository's own
[`no-personal-data`](.claude/rules/no-personal-data.md) rule forbids
shipping one. When a role address exists on the project's domain it goes
here, next to the advisory link rather than instead of it.

**What to expect:** an acknowledgement, a fix or a documented reason not
to fix, and credit in the advisory unless you ask otherwise. This is a
small project, so the honest answer about timelines is that there is no
staffed rotation; a report is read by a person who may be asleep.

## What is in scope

The parts where a mistake crosses a trust boundary:

- **The pairing and identity model** — per-machine keys, the
  `authorized` list, and the `enforce` switch
  ([`specs/017`](specs/017-product.md), section 1).
- **The screen and input transport** — QUIC between machines, and input
  injection, which by design can type anywhere on the target
  ([`specs/010`](specs/010-remote-desktop.md)).
- **The local HTTP API** on `127.0.0.1:8787`, including anything that
  escapes localhost.
- **The updater** — it downloads a published package, verifies it and
  replaces running binaries ([`specs/015`](specs/015-ota.md)).
- **The setup paths that need elevation** — scheduled tasks, launchd
  agents, firewall rules, `udev`.
- **Anything a script authorizes on a machine that runs it.** We shipped
  exactly that defect: two setup scripts carried a literal SSH public key
  and installed it into `authorized_keys`, so following the README gave a
  stranger access to your own machine. It is fixed, it is written up in
  [`specs/018`](specs/018-open-source.md), and reports of that shape are
  very welcome.

## What is not a vulnerability

- **The local signing identity is self-signed and its password is on
  disk.** It exists so macOS keeps a TCC grant across rebuilds; it signs
  nothing another machine trusts. The password is a random value in a
  `600` file, read at runtime
  ([`no-secrets-in-source`](.claude/rules/no-secrets-in-source.md)).
- **Input injection types anywhere on the target.** That is the product.
  The boundary is who may connect, not what they can do once connected.
- **The specs name real machines and describe the topology.** Deliberate:
  a measurement without a real machine on a real day is an anecdote.
  Obscurity is not a control here, but note that defaults and scripts
  never carry those names.
- **Windows Defender heuristics.** `Trojan:Win32/Bearfoos.A!ml` on an
  unsigned binary and `Trojan:Win32/PowExcEnv.H!MTB` on our own
  `-EncodedCommand` are model guesses, not findings. The binaries are
  Authenticode-signed before they are copied, for that reason
  ([`specs/008`](specs/008-security.md)).

## Where the keys live, and where they do not

The Authenticode key never leaves the machine that signs, and the macOS
signing identity never leaves its keychain. **CI deliberately does not
sign and does not install** — a published package is unsigned by design,
and the receiving machine signs it locally after verifying the checksum
([`specs/008`](specs/008-security.md),
[`specs/015`](specs/015-ota.md)).
