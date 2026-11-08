# Nothing shipped identifies a person, a machine or a household

This started as a tool for three machines in one house, and the names of
those machines are still in 22 files. Going open source changes who reads
them: strangers, on hardware we have never seen. The rule draws the line
where it actually matters, because a blanket ban would delete the evidence
that makes the specs worth reading.

## Where names may appear, and where they may not

| Place | Allowed | Why |
|---|---|---|
| `specs/`, `docs/` narrative | **Yes**, under a stable alias | A measurement without a real machine on a real day is an anecdote. "The Windows machine" beats a hostname; a hostname beats a lie. |
| Code, defaults, config templates | **No** | A default is inheritance: the next user runs it. |
| Scripts a user executes | **No** | Same, plus it decides what happens on *their* machine. |
| Commit messages, PR bodies | Yes, as history | Already written, and rewriting history costs more than it buys. |

The distinction is **evidence versus inheritance**. A spec saying "the
Windows IP changed once and broke everything pinned to it" is evidence. A
`default-config.toml` that names three real machines, their mDNS addresses
and their SSH aliases is inheritance — and it is a bug for the second
deployment, not only a leak from the first.

## What counts as personal

Hostnames, user accounts, home directory paths, mDNS names, tailnet names,
real names, e-mail addresses, physical locations, and anything whose only
purpose is to say *which* machine. Public SSH keys count too, for the
reason in [[no-secrets-in-source]].

Measured on 2026-11-08, before any of this was cleaned:

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
a template nobody can use, and it invites someone to fill it back in.

## See also

- [[no-secrets-in-source]] — the same audit, the other half
- [[config-per-machine]] — why the config is per machine and never copied
- [[../../specs/018-open-source]] — the plan this rule serves
