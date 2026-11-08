# 018 — Open source

> Written for: the engineers who will do this work, and the reviewers who
> will tell them it is not done yet.

The decision is made: oxdesk becomes an open source product in a public
repository. This spec is the plan of record. It says what "publishable"
means here, what the audit found, in what order the work happens, and —
on purpose — the strongest case against each part of it.

Publishing is not a release step. It is a change of audience: from one
person who knows which machine is which, to strangers who will run this
code on hardware we have never seen. Everything below follows from that
one sentence.

## Why now

CI stopped working on 2026-11-08 because the organization's Actions
allowance ran out, and standard GitHub-hosted runners are free in public
repositories. The numbers are in [[013-ci-cd]]; the short version:

| | |
|---|---|
| November, product `actions` | gross **US$ 12.0996**, discount US$ 12.0996, net US$ 0.00 |
| The Free allowance | 2,000 Linux minutes × US$ 0.006 = **US$ 12.00** |
| The macOS job's share | US$ 9.38 of 12.00, from 151 minutes of wall clock |

Money is the trigger, not the reason. The reason is that a product meant
to scale cannot have a build that stops mid-month, a first run that
configures someone else's house, and a setup script that installs a
stranger's SSH key.

**What the documentation actually says**, because the premise rests on it:
GitHub lists "standard GitHub-hosted runners in public repositories" among
the free usage types and says that usage "remains free independently and
does not reduce the included compute usage". It says *free*; it does not
say *unlimited*. Larger runners "are billed for public repositories". Our
matrix uses `ubuntu-24.04` and `macos-15` — both standard, so inside the
free rule, and any move to a larger runner leaves it.

## What public also removes

One simplification worth planning for, found while writing
[[../.claude/skills/audit-before-publishing]]: **the OTA token stops being
necessary.** Today the Releases are private, so every machine carries a
fine-grained PAT with `Contents: Read-only` just to download its own
update, and [[../.claude/skills/enable-ota]] opens with "the repo is
private, so without a token the API answers 200 with `Not Found`". A
public repository has public Releases: the token, its distribution script,
its `chmod 600` and its expiry date all go away.

That is the one piece of the move that makes the product *simpler* rather
than more careful, and it is **step 11: after the flip, never before.**
Measured on 2026-11-08, with the token file moved aside on the Ubuntu
machine:

```
erro: o GitHub respondeu: Not Found (repo privado sem token:
      ponha um PAT de leitura em ~/.config/oxdesk/ota-token)
```

Exit code 1, so it fails loudly — the token is still load-bearing while
the Releases are private, and deleting the plumbing today would stop OTA
on all three machines.

The good news is that it is a deletion and not a redesign: the token is
`Option<String>` the whole way down (`apply::read_token` →
`exec::api`/`exec::download` → `exec::token_config`, which already returns
`None` for absent and for blank). What goes, once public:

| What | Where |
|---|---|
| Reading the token | `ota/src/apply.rs` (`read_token`), `ota/src/plan.rs` (`token_path`) |
| Sending it | `ota/src/exec.rs` (`token_config`, and the `Option` parameters) |
| Distributing it | `scripts/distribute-ota-token.sh`, and the two "OTA token" steps in `deploy.sh` |
| Checking it | the wizard's `ota` step (`setup_common::ota`, `setup_linux::ota_token`) |
| Documenting it | section 1 of [[../.claude/skills/enable-ota]], and the token part of [[015-ota]] |

*Check:* an OTA run on a machine with **no token file at all** answers
`igual` instead of `Not Found`.

## What "publishable" means

Three properties, each enforced by a rule so that review has something to
point at rather than an opinion:

| Property | Rule |
|---|---|
| Nothing in the repo identifies a person, a machine or a household | [[../.claude/rules/no-personal-data]] |
| No credential, key or authorized identity lives in source | [[../.claude/rules/no-secrets-in-source]] |
| Every word a contributor reads is English | [[../.claude/rules/english-only]] |

A fourth property is not new, only now load-bearing: a protection that
stops protecting has to fail loudly. Public repositories attract the
reading of code, and "it is all public anyway" must never become the
argument for weakening a gate ([[../.claude/rules/verify-what-arrived]]).

## The license: AGPL-3.0-or-later, and why it was not a preference

The crates declared `license = "MIT"`. That was not sustainable, and the
reason is concrete rather than ideological.

**We already distribute GPL-3.0-or-later code.** `oxdesk-host` and
`oxdesk-view` on macOS link Homebrew's `ffmpeg@7` dynamically — Homebrew
reports it as `GPL-3.0-or-later`, built `--enable-gpl --enable-version3`,
and the Mac package in every Release carries binaries with those dylib
paths recorded. A binary linked against a GPL-3 library cannot be
distributed as MIT and mean it. AGPL-3.0 **is** compatible with GPL-3.0,
so this move makes the publication coherent instead of creating a new
constraint. Linux and Windows packages carry no FFmpeg, so this is a Mac
fact that decides the whole project's license, because the project ships
one Release.

The usual AGPL argument applies too, and it is the weaker of the two here:
section 13 obliges whoever offers the software over a network to publish
their modifications, which is aimed exactly at
`oxdesk-encontro` — the rendezvous and relay of [[017-product]], which an
Enterprise customer may host themselves. For the desktop side, where the
user runs the binary on their own machine, AGPL and GPL are
indistinguishable in practice.

**`hidpp` stays MIT.** It speaks HID++ 2.0 over raw HID, it links nothing
GPL, and it is the one piece with value to projects that are not this one.
AGPL on a small protocol crate buys nothing and blocks the only realistic
form of outside reuse — which is the form most likely to produce a
contributor. If the patent grant matters, the Rust-ecosystem norm is
`MIT OR Apache-2.0` and that is a one-line change.

Every Rust dependency is permissive (`MIT`, `Apache-2.0`, `Unicode-3.0`,
`Unlicense`; no GPL-2.0-**only** anywhere, which would have been
incompatible). Details in [[../THIRD-PARTY]].

### Open question: a CLA, decided now or not at all

AGPL is normally paired with a contributor licence agreement when the
owner wants to offer the same code under other terms — which is the
standard business model behind an Enterprise relay. Without a CLA, we hold
only AGPL rights to contributed code, so **we could not offer a
proprietary licence either**, and collecting agreements retroactively from
contributors is the kind of task that does not finish.

This spec does not decide it. It records that the window is now: the first
outside contribution closes it.

## What the audit found

Measured on 2026-11-08 over the working tree and all 233 commits (2.44 MiB
packed, so the history scan was exhaustive, not sampled).

### Blocker 1 — two scripts install the maintainer's SSH key

`scripts/enable-mac.sh` and `windows-ssh.ps1` carried a literal
`ssh-ed25519 …` public key and **authorized it on the machine that ran
them**. A public key is not a secret and publishing it grants nobody
anything; running the script does. A stranger following the README would
have given a stranger SSH access to their own machine.

This is the finding that would have been worst to publish, and it is not a
privacy problem — it is a backdoor by accident. The key now comes from the
person running the script, and the script refuses with no argument.

### Blocker 2 — the keychain password is in source, twice, and the copies disagree

| Where | Value |
|---|---|
| `mac/create-identity.sh` | `KCPASS="logi"` — creates the keychain |
| `mac/sign.sh` | `unlock-keychain -p logi` — agrees |
| `oxdesk/src/setup_mac.rs` | `unlock-keychain -p "oxdesk"` — **disagrees** |

That keychain holds the Mac's code-signing identity. The second half is a
bug the rename left behind, independent of publishing: the Rust path
unlocks with `oxdesk` a keychain created with `logi`, and the result is
`let _ = run_timed(...)` — **discarded**. It has been failing silently
since the rename, and nothing noticed because the LaunchAgent runs
`sign.sh`, which still has the matching word. Same family as the
`langfuse` and `backfill_gate` no-ops: a protection that stopped working
and said nothing.

### Blocker 3 — the first run configures one house

`oxdesk/default-config.toml` is what a brand-new installation gets, and it
names these three machines, their mDNS addresses and their SSH aliases. A
tool whose first run writes someone else's address book leaks in both
directions: our names out, and a new user's setup wrong.

The spread, by file:

| Identifier | Files | Where |
|---|---:|---|
| `ubuntu-host` | 22 | 9 specs, 5 `oxdesk`, 2 `stream`, scripts |
| `windows-host` | 20 | 8 specs, 7 `oxdesk`, README |
| `windows-host` | 14 | scripts, specs |
| `/home/user` | 9 | 6 `oxdesk`, README, gui |
| `C:\Users\user` | 6 | |
| `/Users/user` | 4 | |

The split decides the work: in **specs** these names are evidence — a real
machine on a real day is what makes a measurement worth reading, and
[[../.claude/rules/no-personal-data]] allows them there under a stable
alias. In **code, defaults and scripts** they are inheritance, and they go.

### Blocker 4 — there is no LICENSE

No `LICENSE`, no `CONTRIBUTING.md`, no `SECURITY.md`. A public repository
with no license is "all rights reserved": nobody may legally use or
contribute, which defeats the point. Cheapest blocker, and the one that
decides whether the rest was worth doing.

### Blocker 5 — CI never runs on a pull request

The workflow triggers on `push` to `main` and `ci/**`, on `v*` tags and on
`workflow_dispatch`. There is no `pull_request` trigger, so a
contributor's PR runs **nothing**. Free on a public repo, and the thing
that makes a contribution reviewable. Conditions in the adversarial
review.

### Blocker 6 — one crate still speaks Portuguese

Only `oxdesk/src`: 15 files, ~241 accented lines of user-facing strings
and messages. `stream`, `ota`, `hidpp`, `agent` and `gui` are already
English. Bounded work, and [[../.claude/rules/english-only]] is what keeps
it from coming back.

## What does **not** need fixing

Written down so nobody repeats the search:

- **No secret of a known shape anywhere in history.** `git grep` across all
  233 commits for `ghp_`, `github_pat_`, `gh[osu]_`, `BEGIN … PRIVATE KEY`,
  `AKIA…`, `xox[baprs]-` and `AIza…`: nothing.
- **No leakage from unrelated work.** No other client, product or
  credential store appears. The 32 files matching a short name are `interface`
  and `surface`.
- **The workflow holds no secrets.** Only `${{ github.token }}`, and the
  release job is gated on `startsWith(github.ref, 'refs/tags/v')`.
- **No private keys and no tailnet names committed.** Only the two public
  keys of Blocker 1.

## History: rotate, do not rewrite

A force-push to clean history was on the table. It is the wrong tool for
what was found.

The only credential in the repo is the keychain password, and it is
**rotatable**: create the keychain with a fresh random value, stop
hardcoding it, and the published string becomes worthless. Rotation makes
history harmless without touching it. A rewrite makes history *different*,
and that costs:

- The specs cite commit hashes as evidence — `git show 9b226cf^` in
  [[017-product]], `aa6d4ed` and `2fc748b` in [[013-ci-cd]]. A rewrite
  turns each citation into a dead reference, and here history is
  documentation.
- A rewrite un-publishes nothing. Anything that ever left this machine is
  gone already; the decision is about future exposure.
- Author emails become public either way, unless those are rewritten too —
  a separate and louder change.

**Decision: rotate the password, parameterize the key, keep the history.**
If a real scanner finds something that cannot be rotated, rewriting is
still available, and then it is the right tool.

## Adversarial review

A decision with arguments on only one side has not been examined. Each
point below is the strongest version of the case against, followed by what
survives it.

### 1. The bill is US$ 12 a month; raising the limit is one click

Making this repo publishable is days of work and the first pass found a
would-be backdoor. On pure cost, paying wins, and paying risks nothing the
audit may have missed.

*What survives:* the US$ 12 is not the problem, the **cliff** is — the
allowance ends mid-month and then nothing builds, which is how the Mac
package became unbuildable at the exact moment one was needed. A spending
limit also fixes the cliff, for money. So this argument stands on cost and
loses on purpose: the work is being done for the audience change, and free
CI is a side effect.

### 2. Nobody can run this

The requirements are a Logitech Unifying receiver with MX devices, an
NVIDIA GPU for NVENC, GNOME on Wayland with Mutter's private ScreenCast
APIs, a Windows machine with a scheduled task under `InteractiveToken`, a
Mac with Homebrew's `ffmpeg@7` — and three of them on one LAN. The
realistic contributor count is near zero, and the genericity work is paid
up front whether or not anyone appears.

*What survives:* this is the strongest objection and effort does not answer
it. Two things do. First, the work is the same work a second deployment
needs — the first new user is us, on different hardware, and Blocker 3 is
already a bug for that case. Second, "contributors" is not the only
benefit: a public record and a build that does not stop are worth having
alone. What this argument should change is the **claim**, not the plan: do
not promise a community, and do not design for one that is not there.

### 3. The repo is a map of a household

It documents which machines exist, their roles and operating systems, the
SSH topology, where the signing identity lives and which flag disables
which protection. None of it grants access; all of it drops an attacker's
reconnaissance cost to zero, for a network inside someone's home.

*What survives:* obscurity is not a control, so this is not a veto — but it
is the reason [[../.claude/rules/no-personal-data]] exists and the reason
specs use a stable alias rather than a hostname. It is also the reason the
machine inventory (`docs/machines.md`) is deleted rather than generalized:
a document whose whole content is "which machines, whose user accounts,
which aliases" has no generic version.

### 4. The audit is pattern matching, not proof

What ran was `git grep` for known secret shapes. It cannot find a
credential that does not look like one: a password in prose, a token in a
URL's query string, a base64 blob in a fixture. It found the keychain
password only because `unlock-keychain` is searchable, and the SSH keys
only because `ssh-ed25519` is.

*What survives:* entirely. Before visibility flips, run `gitleaks` or
`trufflehog` over the full history and attach the report to the PR, and
turn on GitHub secret scanning and push protection — both free on public
repos, and a real benefit of the move rather than a cost.

### 5. Public plus a `pull_request` trigger means running strangers' code

The trigger that makes contribution possible also lets forks start our
jobs.

*What survives:* the blast radius is small today and must be kept that way,
on purpose:

- never `pull_request_target`, which runs with the base repository's token;
- the release job stays tag-only;
- fork PRs get read permissions only;
- any future step that needs a secret must be unreachable from a fork.

### 6. The irreversible part is the rewrite, not the visibility

Visibility can be flipped back. A rewritten history cannot be
un-rewritten once tags and clones have moved, and the specs' citations
break the moment it happens.

*What survives:* it is the argument for the decision above — rotate, keep
history. If the move has to be reversed, reversing a switch is cheaper
than reversing a rewrite.

### What would change the plan

- A scanner finding a credential that **cannot** be rotated → rewrite
  before publishing, or publish a fresh repository and keep this one as
  the archive.
- GitHub documenting a cap on public-repo minutes that we would hit → the
  premise is gone; pay for private minutes instead.
- A decision that the household map should not be public at all → stay
  private, raise the spending limit, and take the Mac off `push`, the cut
  [[013-ci-cd]] already names.

## Order of operations

Each step ends in something checkable, because "it should be fine" is not
a measurement.

1. **Parameterize the SSH key** in `enable-mac.sh` and `windows-ssh.ps1`.
   *Check:* with no argument they refuse instead of installing anybody's
   key. **Done 2026-11-08.**
2. **Delete what has no generic version:** the machine inventory, the
   prototype whose finding already lives in [[005-input-switch]], the
   stale desktop entry pointing at a personal path. *Check:* `git grep`
   for the deleted paths returns only history. **Done 2026-11-08.**
3. **Rotate the keychain password. Done 2026-11-08.** 32 random
   characters generated **on the Mac** (so it never travelled in an `ssh`
   argument), written to `~/.config/oxdesk/keychain-password` with
   `umask 077`, and applied with `security set-keychain-password -o logi`.
   The identity's name and the keychain's filename still say `logi` on
   purpose: both are baked into the certificate's designated requirement,
   and recreating it to rename them would make macOS discard the Screen
   Recording grant. *Checked:* signing a throwaway copy gives
   `Authority=logi local signing` — with `codesign -dvvv`, because `-dv`
   does not print that line; and the only remaining mentions of the old
   literal are this table and the rule, which is what "rotate, don't
   rewrite" means in practice.
4. **Make the default config generic.** Placeholders, or the machine's own
   hostname. *Check:* a fresh config on a machine with a different
   hostname describes that machine.
5. **Purge personal identifiers from code and scripts**, leaving the specs
   as the record under an alias. *Check:* the grep table above reads zero
   outside `specs/`.
6. **Translate `oxdesk/src`.** *Check:* no accented line outside test
   fixtures and specs.
7. **Add `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`.** `LICENSE`
   (AGPL-3.0), `hidpp/LICENSE` (MIT) and [[../THIRD-PARTY]] are **done
   2026-11-08**; the crate metadata moved with them. *Check:* GitHub shows
   the license in the sidebar once public, and `grep -h '^license' */Cargo.toml`
   shows AGPL everywhere except `hidpp`.
8. **Run a real secret scanner over all history.** *Check:* clean report
   attached to the PR that flips visibility.
9. **Add the `pull_request` trigger** under the constraints of adversarial
   point 5, and put `check.sh` in it. *Check:* a throwaway PR runs the
   suite and the release job stays skipped.
10. **Flip visibility; enable secret scanning and push protection.**
    *Check:* a tag build runs the macOS job with US$ 0.00 billed — the
    point of the exercise, and the first chance to learn whether "free"
    means "unlimited".

## See also

- [[013-ci-cd]] — the exhausted allowance, measured, and the by-hand
  release recipe that works while CI is down
- [[008-security]] — why the signing keys never leave their machines
- [[017-product]] — the rename, and what a blind replacement did to this
  repo's own record
- [[../.claude/rules/no-personal-data]], [[../.claude/rules/no-secrets-in-source]],
  [[../.claude/rules/english-only]] — the three rules this spec installs
- [[../.claude/skills/audit-before-publishing]] — the procedure, in the order
  that found the real problems
