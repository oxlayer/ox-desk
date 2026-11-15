# Contributing to oxdesk

Thank you for your interest. There are two things to read before anything
else, then the practical part.

## What you need to know about the licence

The project is **AGPL-3.0-or-later** ([`LICENSE`](LICENSE)), except
[`hidpp/`](hidpp/), which is MIT so other projects can use the HID++
protocol work.

**By submitting a contribution you keep your copyright and grant oxlayer a
perpetual, worldwide, non-exclusive, royalty-free, sublicensable licence
to use, reproduce, modify and distribute it under any licence terms.**

That last word is deliberate and we would rather be plain about it than
bury it: it means we can offer the project, including your contribution,
under terms other than AGPL in the future — for example a commercial
licence for the hosted relay. The reasoning is in
[`specs/021`](specs/021-licensing-and-contributions.md), including the
argument against doing it.

What that grant does **not** let us do: take anything back. An AGPL grant
already made is perpetual and irrevocable for the version it was made
for. Any licence change is forward-only — every published release, tag and
clone stays available under the licence it shipped with, including this
one.

If that is not acceptable to you, say so in the issue before writing code.
A patch nobody can merge wastes your time, not ours, and we would rather
tell you up front.

### How you agree

Two things, and one commit does both:

1. **Add a line to [`.cla/signed`](.cla/signed)** in your pull request:

   ```
   <your-github-login> <terms-version> <date, yyyy-mm-dd>
   ```

   The check prints the exact line to paste, with today's date, so there
   is nothing to look up. That line is the record of the grant above, and
   the middle field is the version of these terms you are agreeing to; if the terms ever change in substance
   the version moves, and an old signature stops counting, because nobody
   agreed to terms that did not exist on the day they signed.

2. **Commit it with `git commit -s`**, which adds a `Signed-off-by` line
   (the [Developer Certificate of Origin](https://developercertificate.org/)).
   That certifies you have the right to submit the code. It is *not* the
   grant above, and it is not a substitute for it; we ask for both.

There is no bot to wait for and no account to connect:
[`.github/workflows/cla.yml`](.github/workflows/cla.yml) reads the file
and tells you exactly what to add if it is missing. The list is
append-only — a pull request that removes or edits an existing line fails
the check, whoever opens it.

If you already have push access here you do not sign anything: being
invited is the agreement, and a maintainer cannot grant themselves a
licence to their own copyright.

The check learns that from GitHub, which tells a workflow that someone is
a member of the organization only when that membership is **public**. A
member whose membership is private is reported as an outside contributor
and asked to sign. Make the membership public (your GitHub profile,
Organizations), or sign like anyone else.

## What you need to know about the hardware

This is not a project you can try on one laptop, and a patch you cannot
run is hard to review:

- **two or more computers on the same LAN**, reachable by mDNS or a
  tailnet name;
- **Linux**: GNOME on Wayland — capture uses Mutter's private ScreenCast
  and RemoteDesktop APIs;
- **Windows**: OpenSSH Server, and the screen host runs from a scheduled
  task with an interactive token;
- **macOS**: Homebrew's `ffmpeg@7`, plus Screen Recording and
  Accessibility granted by hand;
- **channel switching**: a Logitech Unifying receiver and devices that
  speak HID++ 2.0 — everything else works without it;
- **NVENC**: an NVIDIA GPU, or the screen path costs more CPU.

If you only have one of the three platforms, say which in the pull
request. Many bugs here only reproduce on one platform.

## How we work

Read [`CLAUDE.md`](CLAUDE.md) first. It indexes
[`.claude/rules/`](.claude/rules/), and every rule in there records a
failure that has already happened at least once. Three are not
negotiable:

| Rule | In one line |
|---|---|
| [`no-personal-data`](.claude/rules/no-personal-data.md) | Nothing in the tree identifies a person, a machine or a household. Specs keep their evidence and name machines by stable aliases (`ubuntu-host`, `windows-host`, `mac-host`); defaults never name one, because the next user inherits them |
| [`no-secrets-in-source`](.claude/rules/no-secrets-in-source.md) | No credential, and no key a script will authorize on someone else's machine |
| [`english-only`](.claude/rules/english-only.md) | Code, comments, commits, logs, errors, UI — one language |

**A measurement beats an opinion.** If you change behaviour, say what you
ran and what it printed. If you fix a bug, say how you reproduced it. If
you could not reproduce it, say that too — "intermittent, four failures
and one pass" is a useful sentence and a guess dressed as a cause is not.

**Write down what failed.** The specs in [`specs/`](specs/) are the real
documentation, and they are full of things that did not work and why.
That is deliberate: it keeps the next person from repeating the same
mistake.

## Before opening a pull request

```bash
bash scripts/check.sh      # builds and tests every crate, native and cross
```

If it fails on the licence step because you added a file, write the header
with `bash scripts/spdx.sh --fix` and read it before committing: it says
AGPL for everything except `hidpp/`, which is MIT.

It has to be green. It builds for Windows too, from Linux, because part of
some crates only compiles there — and code for another OS passes
`cargo build` here by not being compiled at all
([`check-the-other-cfg`](.claude/rules/check-the-other-cfg.md)).

Commit messages in English, in the imperative, saying **why**. The
history here is documentation; a message that only restates the diff adds
nothing a diff does not already say.

## Reporting a security issue

Do not open a public issue. See [`SECURITY.md`](SECURITY.md).
