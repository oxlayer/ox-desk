# 013 — Binaries via CI

> Status: implemented. Linux and Windows checked against what this
> Ubuntu compiles; the Mac package was checked by format and stamp,
> but **has not run on the MacBook yet** (it was offline that day).

## The problem

The binaries only existed as a side effect of `deploy.sh`: compiled
on the machine of whoever triggers it, and the Mac compiling from an `rsync`.
If the Mac is offline, nobody finds out that its code stopped
compiling. And there was no way to answer "which binary corresponds to this
commit?" without recompiling.

## The shape

One workflow, [`binarios.yml`](../.github/workflows/binarios.yml), which
calls one script, [`construir-binarios.sh`](../scripts/construir-binarios.sh).

| Target | Runner | How it compiles | Package |
|---|---|---|---|
| Ubuntu | `ubuntu-24.04` | native, and runs the tests | `logi-linux-x86_64-<n>-<hash>.tar.gz` |
| Windows | `ubuntu-24.04` | cross, `x86_64-pc-windows-gnu` | `logi-windows-x86_64-unsigned-<n>-<hash>.tar.gz` |
| macOS | `macos-15` (arm64) | native, ffmpeg from brew | `logi-mac-arm64-unsigned-<n>-<hash>.tar.gz` |

| Event | What happens |
|---|---|
| push to `main` or `ci/**` touching code | compiles all three, packages kept 14 days as artifacts |
| tag `v*` | the same, and publishes a Release with the three packages and `SHA256SUMS` |
| `workflow_dispatch` | the same as push, on demand |

A documentation-only commit triggers nothing: the `paths` filter exists
so as not to spend minutes on that.

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
- `logid` does not carry the commit stamp, the same question the
  deploy asks `/healthz`, only before any machine receives it;
- on Linux, `ldd` reports a missing library;
- on Windows, the `.exe` imports a DLL the system does not have (mingw, ffmpeg);
- on the Mac, `logid` depends on brew.

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

**`construir-binarios.sh` runs locally.** The binary list cannot
live only in a YAML nobody executes. `--sem-compilar` checks what is already
in `target/`.

## Known limits

- **The binary list is duplicated** between `deploy.sh` and
  `construir-binarios.sh`. A new binary goes into both. It is the defect that
  already left `logistream-kvm` out of a deploy; merging the two lists
  is deferred until the deploy starts consuming the packages.
- **The Mac host and view link against brew's ffmpeg.** The run used
  9.0.2. The package only runs with the same major installed on the MacBook, which
  was not checked.
- **Tests run only on Linux**, and for `logistream` only `--lib`. On the Mac
  it would cost ten times more per minute.
- **The conversion budget does not block CI.**
  `conversao_cabe_no_orcamento` passed on the first run and failed on the third
  with 12.18 ms against the 12 ms ceiling, with no change in the measured code: a shared
  runner is not the hardware the ceiling was written for. It
  keeps running and printing the number, without deciding. Since `check.sh`
  does not test `logistream` either, **today nothing automatic blocks this
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

## See also

- [[008-security]] — why the key is not uploaded
- [[../.claude/skills/deploy]] — the path to the machines
- [[../.claude/rules/verify-what-arrived]]
