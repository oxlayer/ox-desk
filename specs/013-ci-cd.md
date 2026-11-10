# 013 — Binaries via CI

> Status: implemented. Linux and Windows checked against what this
> Ubuntu compiles; the Mac package was checked by format and stamp,
> but **has not run on the MacBook yet** (it was offline that day).

> ⚠️ **Red since 2026-11-08 03:10, for a reason outside this repo.** Every
> run from `2fc748b` on fails in 2-6 s with **zero steps executed**: no
> checkout, no logs, nothing to read. The last green was `bbae0a6` at
> 02:51 the same night, and the two commits in between touched neither
> `.github/`, nor `BUILD_BASE`, nor `build-binaries.sh` -- same workflow,
> same script, green and then dead. Actions is enabled and all actions are
> allowed, so what is left is the account side, and the Actions **spending
> limit** is the prime suspect (this is a private repo, and `macos-15`
> bills at ten times a Linux minute). Checking it needs account scopes
> that a repo token does not carry: it is at
> <https://github.com/settings/billing>. Until then CI reports nothing
> about the code, and `deploy.sh`'s own `check.sh` is the only gate that
> actually runs.

## The problem

The binaries only existed as a side effect of `deploy.sh`: compiled
on the machine of whoever triggers it, and the Mac compiling from an `rsync`.
If the Mac is offline, nobody finds out that its code stopped
compiling. And there was no way to answer "which binary corresponds to this
commit?" without recompiling.

## The shape

One workflow, [`binaries.yml`](../.github/workflows/binaries.yml), which
calls one script, [`build-binaries.sh`](../scripts/build-binaries.sh).

| Target | Runner | How it compiles | Package |
|---|---|---|---|
| Ubuntu | `ubuntu-24.04` | native, and runs the tests | `oxdesk-linux-x86_64-<n>-<hash>.tar.gz` |
| Windows | `ubuntu-24.04` | cross, `x86_64-pc-windows-gnu` | `oxdesk-windows-x86_64-unsigned-<n>-<hash>.tar.gz` |
| macOS | `macos-15` (arm64) | native, ffmpeg from brew | `oxdesk-mac-arm64-unsigned-<n>-<hash>.tar.gz` |

| Event | What happens |
|---|---|
| push to `main` or `ci/**` touching code | compiles all three, packages kept 14 days as artifacts |
| tag `v*` | the same, and publishes a Release with the three packages and `SHA256SUMS` |
| pull request to `main` touching code | compiles all three, before the merge |
| `workflow_dispatch` | the same as push, on demand |

A documentation-only commit triggers nothing: the `paths` filter exists
so as not to spend minutes on that. The pull-request trigger repeats that
same list instead of sharing it, because Actions does not resolve YAML
anchors -- and a docs-only PR that spent a `macos-15` runner would be
paying ten times a Linux minute for nothing.

`push` stays limited to `main` and `ci/**`, so a pull request from an
ordinary branch runs once, not twice.

## What CI does not do

**It does not sign.** The Authenticode key does not leave this machine
([`008`](008-security.md)), and uploading the key to a GitHub secret
would trade the trust model for convenience. The Mac identity
lives in a keychain over there. That is why the Windows and Mac packages carry
`unsigned` in the name:

- a raw `.exe` arriving on Windows is what Defender deletes on write;
- an ad-hoc binary on the Mac loses the Screen Recording grant.

**It does not install.** `deploy.sh` remains the only path to the
machines: it signs, swaps, restarts and checks `/healthz`. Making CI
deliver would require a runner inside the LAN or a port open to the outside,
and the rule is everything on the LAN. The exception in
[`012`](012-outside-the-lan.md) is narrow and does not cover this.

The no-cloud rule ([`008`](008-security.md)) applies to the running
system: no data from the machines goes through GitHub. What leaves here
is the code, which was already there.

**It does not package the Electron shell.** `gui/` has no logic, and the deploy
only takes `main.js`.

## What is checked

"It compiled" is not enough ([[../.claude/rules/verify-what-arrived]]). Before
packaging, the script refuses if:

- any binary on the list is missing, or is smaller than 100 KB;
- the format is not the target's (ELF, PE32+, Mach-O arm64);
- `oxdesk` does not carry the commit stamp, the same question the
  deploy asks `/healthz`, only before any machine receives it;
- on Linux, `ldd` reports a missing library;
- on Windows, the `.exe` imports a DLL the system does not have (mingw, ffmpeg);
- on the Mac, `oxdesk` depends on brew.

The size and sha256 of each binary go to the log and the run summary. The
wrong stamp was tested on purpose: `deadbee` is refused and nothing is
packaged.

## Decisions

**Cross-compiled Windows, not `windows-latest`.** It is the same path as
`deploy.sh`, so the CI binary is the same kind as what already runs there. A
Linux minute also costs half of a Windows one.

**Tar, not a loose binary.** `upload-artifact` loses the execute
bit. Checked in the downloaded package: `-rwxr-xr-x`.

**Rust pinned at `1.99.1`**, the version on this Ubuntu. It goes up on purpose,
together with the machine, and not on its own on a Tuesday.

**Actions pinned by SHA.** An action tag is mutable, and the release job
has write permission. `contents: write` exists only there.

**`ubuntu-24.04` runner on an Ubuntu 26.04.** Compiling against the older glibc
runs on the newer one; the reverse does not.

**`build-binaries.sh` runs locally.** The binary list cannot
live only in a YAML nobody executes. `--sem-compilar` checks what is already
in `target/`.

## Known limits

- **The binary list is duplicated** between `deploy.sh` and
  `build-binaries.sh`. A new binary goes into both. It is the defect that
  already left `oxdesk-kvm` out of a deploy; merging the two lists
  is deferred until the deploy starts consuming the packages.
- **The Mac host and view link against brew's ffmpeg.** The run used
  9.0.2, and the package only runs with the same major installed on the
  MacBook. That IS checked now, on both ends: `FFMPEG_SONAME` in
  `build-binaries.sh` breaks the build if the major moves, and `check_links`
  in the OTA refuses a package whose `/opt/...` paths do not exist on the
  machine. What remains is that the MacBook **cannot hold the `ffmpeg@7`
  keg**: macOS 14 is Tier 3 for Homebrew, there is no bottle, and building
  from source fails there (`make: *** [frameworkinstallapps] Error 2`,
  measured 2026-11-08). So `opt/ffmpeg@7` is an alias to the same keg as
  `opt/ffmpeg`, anything built there records the UNVERSIONED path, and the
  package path rewrites the recorded paths to the versioned prefix with
  `install_name_tool` — only after measuring that both prefixes are the same
  inode, and refusing if they are not. A package has to pin the versioned
  prefix because that is what makes it installable on a machine that has only
  the keg, which is every runner and the next Mac.
- **Tests run only on Linux**, and for `oxdesk-stream` only `--lib`. On the Mac
  it would cost ten times more per minute.
- **The conversion budget does not block CI.**
  `conversion_fits_the_budget` passed on the first run and failed on the third
  with 12.18 ms against the 12 ms ceiling, with no change in the measured code: a shared
  runner is not the hardware the ceiling was written for. It
  keeps running and printing the number, without deciding. Since `check.sh`
  does not test `oxdesk-stream` either, **today nothing automatic blocks this
  regression**: whoever touches the conversion runs the test here.
- **Without a Mac runner there is no release.** In the tag test, the job waited 15
  minutes without a runner and was cancelled (`The job was not acquired by
  Runner`, with a capacity shortage warning for macOS arm64). It is not our
  error and it leaves no log. The release waits for all three, on purpose: the
  fix is `gh run rerun <id> --failed`, tested, and the Mac passed in
  1min10.
- **Tests on a runner without a GPU do not cover capture or NVENC.** Whatever depends
  on a graphical session is still verified on the machines.

## Publishing a version

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The tag must start with `v`. Tested end to end with a throwaway
tag: four attachments on the release, checked by the job itself and
again by `sha256sum -c` on the downloaded package.

## Cost

The repo is private on the organization's Free plan: 2,000 minutes per month, and
a macOS minute counts **ten times**. Each job is rounded up.

Measured on the first run, without cache (2026-11-05):

| Job | Duration | Minutes billed |
|---|---|---|
| linux, with 116 tests | 5min09 | 6 |
| windows | 3min45 | 4 |
| mac | 2min24 | 3 × 10 = 30 |
| **total** | | **40** |

With cache, measured on the run of `37c4983`:

| Job | Duration | Minutes billed |
|---|---|---|
| linux, with tests | 5min30 | 6 |
| windows | 1min10 | 2 |
| mac | 1min35 | 2 × 10 = 20 |
| **total** | | **28** |

That is 50 to 70 code pushes per month. The Linux job barely gains
from the cache: the tests recompile in release. The Mac is more than two thirds
of the bill. If the budget gets tight, the first cut is taking it off
push and leaving it on tag and `workflow_dispatch`.

### When the minutes run out (2026-11-08)

The allowance ended between 02:51 and 03:10, and from then on **every job
fails in 2 to 3 seconds**: `runner_id: 0`, no runner name, zero steps, no
log. Worth telling apart from the capacity case above, which *waits* and is
cancelled after 15 minutes — here nothing is ever scheduled.

What rules out our own code: the workflow file is byte-identical between the
run that succeeded at 02:37 and the one that failed at 03:10 on the same
branch (`git diff aa6d4ed 2fc748b -- .github/workflows/`), and `gh run rerun`
reproduces it on attempt 2. GitHub's own status page was fully operational.

**The cause, measured.** The old billing endpoints are gone (`410`, and they
asked for `admin:org`), but the enhanced-billing one answers with plain
`read:org`:

```bash
gh api "organizations/oxlayer/settings/billing/usage?year=2026&month=11"
```

November 2026, product `actions`: gross **US$ 12.0996**, discount **US$
12.0996**, net **US$ 0.00**. The Free allowance is 2,000 Linux minutes, and
at the current US$ 0.006 per Linux minute that is exactly **US$ 12.00** — so
the month crossed the line. The net is zero not because it fit, but because
the spending limit is US$ 0: past the allowance GitHub stops scheduling
instead of charging, which is the two-second failure above.

October, for contrast: 1,733 minutes, US$ 10.47, all discounted, never
blocked.

Where it went in November:

| repo | Linux | macOS | macOS in US$ |
|---|---:|---:|---:|
| ox-desk | 283 min | 151.3 min | 9.38 |
| ox-code | 154 min | — | — |

The cost table above predicted this shape: **the Mac is 78% of the bill**
(US$ 9.38 of 12.00) out of 151 minutes of wall clock, because a macOS minute
bills ten times. Three ways out, in order of cost: raise the spending limit
above US$ 0 (the whole month was US$ 12); take the Mac off `push`, which is
the first cut named above; or wait for the allowance to reset on the 1st.

The first cut named in **Cost** above — taking the Mac off `push` and leaving
it on tag and `workflow_dispatch` — is still the cheapest lever, and it is a
job-level `if`, not a new workflow.

### Publishing a Release without CI

The fleet is kept current by `deploy.sh`, so a dead CI blocks only OTA — and
OTA is what the Mac has when it is away from home. The release job is a
handful of commands, and they run here:

```bash
bash scripts/build-binaries.sh linux
bash scripts/build-binaries.sh windows
# `deploy.sh` rsyncs the crates to the Mac and NOT `scripts/`, so the script
# has to travel too:
ssh mac 'mkdir -p ~/oxdesk/scripts'
scp scripts/build-binaries.sh mac:oxdesk/scripts/
# No cross-compile to macOS, and the copy over there has no .git, so the
# stamp travels in the environment:
ssh mac 'cd ~/oxdesk && PATH=$HOME/.cargo/bin:/opt/homebrew/bin:$PATH OXDESK_BUILD=<hash> OXDESK_BUILD_N=<n> bash scripts/build-binaries.sh mac'
scp mac:oxdesk/dist/oxdesk-mac-*.tar.gz dist/
# In a directory holding ONLY the three packages of THIS build: `dist/` keeps
# old ones, and the release job requires exactly three.
sha256sum *.tar.gz > SHA256SUMS
gh release create v0.2.8 *.tar.gz SHA256SUMS --verify-tag --generate-notes
```

Done this way for **v0.2.8** (#229 `b5ffb99`), after which the three machines
answered `igual: em dia no build #229 b5ffb99`. The packages are unsigned,
exactly like CI's: the Mac signs with its local identity after downloading,
and the Windows key never leaves the LAN ([[008-security]]).

## See also

- [[008-security]] — why the key is not uploaded
- [[../.claude/skills/deploy]] — the path to the machines
- [[../.claude/rules/verify-what-arrived]]
