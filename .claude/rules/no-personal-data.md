# Nothing in the tree identifies a person, a machine or a household

This started as a tool for three machines in one home. Going open source
changes who reads the tree: strangers, on hardware we have never seen.
The rule is broad in where it applies and narrow in what it removes:
**real machine, user and network names appear nowhere in the tree** —
code, scripts, specs, docs and rules alike — while the evidence they were
attached to stays, written against stable aliases. A blanket ban on the
evidence would delete what makes the specs worth reading; a ban on the
names costs nothing.

## Where names may appear, and where they may not

| Place | Allowed | Why |
|---|---|---|
| `specs/`, `docs/`, `.claude/` | **No** — use the aliases below | A measurement without a real machine on a real day is an anecdote, so the machine stays, by role. "The Windows machine" or `windows-host` carries the evidence; the hostname only ever added identity. |
| Code, defaults, config templates | **No** | A default is inheritance: the next user runs it. |
| Scripts a user executes | **No** | Same, plus it decides what happens on *their* machine. |
| Commit messages and PR bodies already written | Yes, as history | Already published, and rewriting history costs more than it buys. |
| New commit messages, PR bodies, issues | **No** — use the aliases | The history is public too, and it is the one place nothing can be fixed after the push. |

The distinction behind the code and script rows is **evidence versus
inheritance**. A spec saying "the Windows IP changed once and broke
everything pinned to it" is evidence, and it needs no hostname to be. A
`default-config.toml` that names three real machines, their mDNS
addresses and their SSH aliases is inheritance — and it is a bug for the
second deployment, not only a leak from the first.

## The aliases

Every spec, doc and rule uses the same names, so a measurement in one file
can be matched to a measurement in another.

| Role | In prose | In commands, configs, logs, code blocks |
|---|---|---|
| The Ubuntu machine | the Ubuntu machine | `ubuntu-host`, `ubuntu-host.local` |
| The Windows machine | the Windows machine | `windows-host` (`ssh windows-host`) |
| The Mac | the Mac | `mac-host`, `mac-host.local` (`ssh mac-host`) |
| A user account | — | `user` (`/home/user`, `C:\Users\user`, `user@ubuntu-host`) |
| LAN addresses | — | `192.0.2.10` (Ubuntu), `192.0.2.20` (Windows), `192.0.2.30` (Mac): RFC 5737 documentation addresses |
| The tailnet | the tailnet | `example.ts.net`, if a name is needed |
| A person | a user, the report, or just the symptom | — |

"The reference deployment" is the phrase for the three machines the
measurements were taken on. Hardware descriptions are not personal and
stay ("an M1 MacBook Air", "an RTX 3050"): they say what was measured,
not whose it was. A quote of what someone said becomes the symptom, in
neutral English.

## What counts as personal

Hostnames, user accounts, home directory paths, mDNS names, SSH aliases,
LAN addresses, tailnet names, real names, e-mail addresses, physical
locations, and anything whose only purpose is to say *which* machine.
Public SSH keys count too, for the reason in [[no-secrets-in-source]].

Measured on 2026-11-08, before any of this was cleaned. It is the
baseline the cleanup worked down from, and under this rule the target for
every row is zero:

| Identifier | Files |
|---|---:|
| the Ubuntu hostname | 22 |
| the Windows hostname | 20 |
| the Windows SSH alias | 14 |
| the Linux home path | 9 |
| the Windows user path | 6 |
| the macOS home path | 4 |

## A default comes from the machine, not from the repo

The first run must describe the computer it is running on. Read the
hostname, ask the wizard, or write a placeholder that refuses to work
until filled — anything but a name we typed. If an example is useful, it
is `*.example.toml` and it is never what the daemon writes on first start.

## Delete what has no generic version

Some files are personal all the way down, and generalizing them produces
an empty document. `docs/machines.md` was a table of three hostnames,
three user accounts and three SSH aliases: there is nothing left after the
edit, so it was deleted and the one rule inside it — never write an IP —
already lives in [[../../specs/007-device-routing]].

Prefer deleting to anonymizing. An anonymized personal document reads like
a template nobody can use, and it invites someone to fill it back in. The
aliases are for evidence inside a document that has substance of its own;
a document whose only content is which machines exist is deleted, not
aliased.

## See also

- [[no-secrets-in-source]] — the same audit, the other half
- [[config-per-machine]] — why the config is per machine and never copied
- [[../../specs/018-open-source]] — the plan this rule serves
