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

### The CLA: decided, in [[021-licensing-and-contributions]]

This spec used to leave it open. It is decided: **a contributor licence
agreement with a sublicensable grant, in place before the repository
becomes public.**

The fact that settled it is that we own everything today — `git log --all`
returns a single author, nothing is vendored, so the right to relicense
exists right now and unconditionally. It is not lost by a decision, it is
lost by one merged pull request we do not own. So the instrument goes in
before step 10, not after.

And one constraint no agreement can fix: the macOS packages link
GPL-3.0-or-later FFmpeg, which binds **what we distribute** regardless of
who owns our source. That makes [[020-native-mac]] a prerequisite for
licensing freedom rather than dependency hygiene.

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

**Corrected, 2026-11-08.** That count was accented lines, which flattered
the estimate twice over. Unaccented Portuguese is wider than the accents
(`nao`, `esta`, `faltam`, `igual`) and reaches `hidpp`, `ota` and
`scripts/`; and the hardest part is not prose at all, it is the **wire
format** -- `serde` renames, request fields, state values, CLI flags.
Those have a reader on the other side, so each has to move with its
consumer in the same commit or the daemon speaks one name and the panel
reads another. The work is bounded but it is four or five coordinated
commits, not one pass over the strings.

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

## History: rotated first, then squashed

This section argued against rewriting, and the argument lost on a point of
fact. Both halves are kept, because the reasoning is the useful part.

**The original position.** The only credential found was rotatable, so
rotation made every published copy worthless without touching history —
and a rewrite would turn each commit hash the specs cite into a dead
reference, in a repo where the history is documentation.

**What changed it.** Two things, in order:

1. **The rotation happened** (step 3), so nothing published in the old
   commits still works: the keychain password is a new random value and
   the SSH public key never granted anything by being published.
2. **The citations survive anyway.** Measured before doing it:
   `9b226cf`, `aa6d4ed` and `2fc748b` — the hashes cited in
   [[017-product]] and [[013-ci-cd]] — are all **ancestors of the
   `v0.2.8` tag**, and there are eleven release tags from `v0.1.0` to
   `v0.2.8`. Git keeps everything a tag reaches, so `git show 9b226cf^`
   still resolves after the branch is rewritten.

So the squash is narrower than it sounds: **it cleans the branch, not the
history.** Anyone who wants the old commits can still walk them from a
tag, which is also why it does not qualify as erasure — and if erasure
were the goal, the tags would have to go too, and with them the
provenance of every published Release. That trade was not taken.

**What it nearly cost, and this one is not about documentation:** the
squash reset `git rev-list --count` from 240 to **1**, and that number is
the build stamp the updater compares. `decide()` in `ota/src/release.rs`
refuses to go backwards (`if n_there < n_here`) and otherwise **applies**
— so with `n_here = 1` against the published package's `n_there = 229`,
every machine would have installed older code over itself on the next
timer tick. Caught before the deploy, not after.

The fix is a `BUILD_BASE` file at the root, added to the commit count by
`deploy.sh`, `build-binaries.sh` and the CI workflow. It is 240 today, so
the first post-squash build is #241 and the sequence stays monotonic.
**Any future history rewrite has to raise it**, and the check is one line:
the number must come out greater than the `n` in the newest published
package name.

**What it does cost, and what was done about it:** the commit messages.
A squash collapses them, and today's carried measurements that were not
anywhere else. Those were moved into documents first —
[[../.claude/rules/windows-uploads]] for the upload failure and its four
dead hypotheses, [[019-one-onboarding]] and [[020-native-mac]] for the
Mac, [[../.claude/rules/verify-what-arrived]] for the pipe that ate three
exit codes. A finding that lives only in a commit message is one rebase
away from gone.

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
4. **Make the default config generic. Done 2026-11-10** (`7ca42b8`).
   `DEFAULT_TOML` is a template now, rendered by `config::initial_config`:
   the local name comes from `hostname`, `os` from the build, `units` only
   on Linux, the peers are commented examples, and `[[devices]]`,
   `border.target` and `follow.leader` name nobody. *Checked:* a fresh
   `XDG_CONFIG_HOME` gets a config describing THIS machine with zero
   surviving placeholders -- and the first run of that check measured a
   stale `target/release/oxdesk`, because `cargo test` does not rebuild
   it. Green tests plus a stale artifact reads exactly like a working
   change.
5. **Purge personal identifiers from code and scripts. Done 2026-11-10**
   (`f971591` code, `db63f57` scripts), leaving the specs as the record
   under an alias. Prose became the role ("the Windows machine"), literals
   became `linux-pc` / `windows-pc` / `the-mac`, fixtures became `alice`.
   Two were not text: a systemd unit the wizard WRITES carried
   `file:///home/<me>/dev/...`, and `oxdesk-view` with no argument
   defaulted to one of our hosts. The scripts' ssh aliases moved to a
   gitignored `scripts/targets.sh` (with `targets.example.sh` committed),
   the deploy REFUSES with no target, the Windows `scp` paths became
   relative to the ssh user's home, and the two firewall scripts now
   require the LAN range instead of defaulting to ours. *Checked:* zero
   outside `specs/` and `.claude/`; the one remaining hit lives in the
   untracked `targets.sh`.
5b. **Reissue the Windows signing certificate.** Its subject is
   `CN=logi (assinatura local), O=infra pessoal` — the old name, in
   Portuguese, saying "personal infra" — and it is **embedded in every
   Windows binary we publish**, valid until 2031. Windows shows the
   publisher name in its own dialogs, so this is user-visible text, not
   metadata. Reissuing means a new key and certificate, re-importing into
   that machine's Trusted Root and Trusted Publishers stores, and
   re-signing; binaries signed with the old one keep working while the old
   certificate stays trusted ([[008-security]]). *Check:*
   `openssl x509 -subject` shows the project's name in English, and
   `Get-AuthenticodeSignature` on Windows still says `Valid`.
6. **Translate what a contributor reads.** Four slices, because the wire
   format moves with its consumer or not at all. *Check per slice:* no
   Portuguese identifier, field name or string left in it, and
   `scripts/check.sh` green.
   - 6a. **The wizard. Done 2026-11-08** (`7dfefbb`): step ids, JSON
     fields, six routes, ~145 measured messages, `setup.html`, and the
     GUI's two consumers. *Checked:* zero accented lines in `setup*.rs`
     except two comments that quote what a pt-BR Windows prints, which is
     the point of them.
   - 6b. **The API contract. Done 2026-11-08** (`a969c09`): 16
     `serde(rename)`, 3 `hidpp` JSON keys, 5 request fields, 4 response
     keys, the copy queue's state values with the panel's CSS classes, and
     ~45 strings in `main.rs` and `hidpp`.
   - 6c. **The panel's visible copy. Done 2026-11-08** (`9bcb407`), which
     this list did not record until 10/11 -- it still described
     "copiando...", "chegou" and "novidades" as pending while
     `index.html` had none of them. *Checked:* zero accented characters
     and zero of those words in the file.
   - 6d. **The CLI and the OTA's vocabulary.** 29 flags and subcommands
     (`--conferir`, `--estado`, `--forcar`, `--curto`, `--pedir`,
     `--probe-nativo`, `kvm-soltar`, `clip-arq-*`, `esquerda/direita`),
     the state words on the wire (`igual`, `mais-novo-aqui`,
     `em dia no build`), and the build stamp's `-sujo`. This one is last
     on purpose: `deploy.sh` stamps `-sujo` and `ota/src/release.rs`
     strips exactly that suffix, so renaming one side changes what an
     already-installed updater concludes about a published build. It
     moves with its tests, or not at all.
   - 6e. **`scripts/`. Done 2026-11-10** (`31e1e68`). 305 replacements
     across eight scripts: shell identifiers, the variables of the
     PowerShell embedded in `deploy.sh`, the `--pular-mac` flag (now
     `--skip-mac`) and the footer a person reads. That footer was guarded
     by a comment saying it is "parsed by other tools and people" -- the
     reader turned out to be exactly one file,
     `.claude/skills/deploy.md`, which moved in the same commit. The
     comment now names it: a prohibition without a reason is one somebody
     steps over, and I had already stepped over it before reading the
     line.

     *The sweep also broke something*, which is why 6f exists below:
     it rewrote `erro` and `novo` INSIDE a `grep -E` pattern in
     `distribute-ota-token.sh` that matches `oxdesk-ota`'s output
     (`6c0181d` undoes it, along with two older defects in that file that
     meant it had never run on Linux at all).

   - 6f. **The error messages of the Rust crates.** 128 strings that 6b
     did not reach, because 6b was scoped to `main.rs` and `hidpp`:
     `"criar {dir}"`, `"gravar o pin"`, `"ler a identidade local"`,
     `"o arquivo nao esta mais la"`, and the whole of `ota/` (`apply.rs`,
     `exec.rs`, `plan.rs`, `release.rs`, `state.rs`). They reach a person
     through the API's `message` field, the wizard and the logs, so they
     are the slice a contributor reads most. Found on 10/11 while sizing
     6d; a slice that exists in the code and not in the plan is the one
     nobody does. *Checked for readers first:* no test and no UI code
     matches on their text -- the GUI smoke tests key on the console
     `level`, not the message.

     `ota/src/state.rs`'s result words (`igual`, `mais-novo-aqui`) are
     NOT in 6f: those are the state vocabulary of 6d and move with it.
7. **Add `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`. Done 2026-11-08.**
   `LICENSE` (AGPL-3.0), `hidpp/LICENSE` (MIT), [[../THIRD-PARTY]],
   [[../CONTRIBUTING]] — which carries the inbound grant of
   [[021-licensing-and-contributions]] — and [[../SECURITY]], which points
   at GitHub private advisories rather than a personal address, because a
   security contact has to be a role and `no-personal-data` forbids
   shipping one. *Check:* GitHub shows the licence in the sidebar once
   public, `grep -h '^license' */Cargo.toml` shows AGPL everywhere except
   `hidpp`, and no link in those files is dangling. Still open from 021:
   SPDX headers, `NOTICE`, and the CLA bot.
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
