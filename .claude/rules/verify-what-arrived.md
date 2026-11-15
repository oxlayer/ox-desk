# Sent is not arrived; a check that measured nothing passes; a number nobody prints, nobody checks

Faces of one rule, each paid for separately: **a step that reports
success has not thereby done its job.** Something can be sent and not
arrive, an exit code can be eaten before anyone reads it, a check can
pass by measuring nothing, and a number nobody prints is a number nobody
checks.

## 1. Every remote step checks what arrived

An `scp` that "worked" doesn't guarantee the right file at the
destination. Defender once **restored the quarantined copy on top of**
the freshly sent `oxdesk-agent.exe` — 402984 bytes where 458792 had been
sent. The deploy said ok.

That's why each Windows step in `deploy.sh` captures stderr, tests the
result and checks the size of the file that landed **in PowerShell**,
not in bash (where the expansion would happen on the wrong side).

Same family: `cargo test` does **not** build the deliverable. Running
the tests and installing while thinking the binary had been updated
already cost an hour of confusion about why the validation wasn't
taking effect.

## 1b. A pipe eats the exit code you were about to trust

`cmd | tail -5` exits with **`tail`'s** status, not `cmd`'s. Three times in
one day this turned a failure into a green line:

| What ran | What it really did | What the pipe said |
|---|---|---|
| `brew install ffmpeg@7 \| tail -25` | failed: `make: *** [frameworkinstallapps] Error 2` | exit 0 |
| `oxdesk-ota --check \| tail -4` | exit 1, repo private without a token | exit 0 |
| `bash scripts/deploy.sh \| tail -32` | exit 1, one step failed | exit 0 |

The deploy was not even at fault: it ends in `exit "$failures"`, and the
pipe discarded it.

So: when the exit code is the thing being checked, do not pipe. Redirect
to a file and read it, or capture into a variable like the steps in
`deploy.sh` do, or set `pipefail` and mean it. A filtered command is for
reading; an unfiltered one is for deciding.

## 1c. A check that measured nothing passes

The exit code can also be honest and still meaningless, because the
thing being checked was never reached. Twice in two days:

| Gate | What it printed | What it had actually measured |
|---|---|---|
| the panel's `binaries` step | `ok — 6 binaries` | that six files exist in `~/.local/bin` — while the daemon that has to *launch* them could not resolve the name, so the hotkey did nothing |
| `scripts/spdx.sh` | `all 0 files declare a licence`, exit 0 | nothing. Its `cd` had failed, `git ls-files` ran in the wrong directory and returned no files |

Both are worse than a missing gate, because `check.sh` and the panel
print `ok` beside them, and `ok` is what stops anyone looking.

The second one was found by taking `shellcheck`'s dullest warning
seriously — SC2164, `cd` without `|| exit`. With `set -uo pipefail` and
no `-e`, a failed `cd` **continues**. Guarding the `cd` fixes one cause;
what fixes the shape is the rule below.

**A gate must be able to say how many things it checked, and zero must be
an error.** A loop over an empty list succeeds in every language. So
print the count, and refuse it when it is zero: `spdx.sh` now exits 1
naming its cwd, because no revision of this repository has ever had zero
source files. Same question for any new check — *what does it print when
it has nothing to look at?* If the answer is `ok`, it is not a check yet.

And measure the side that fails. Files existing is not the same claim as
a process being able to launch them, which is why the panel now asks the
daemon's own resolver instead of the filesystem
([[graphical-session]]).

## 2. A number that matters must appear in the log

The NVENC bitrate target was wrong for weeks — the formula fell to the
8 Mbps floor at any resolution while the comment promised 15 and 34. It
survived because **only the Mac host printed the target**. When the
Retina screen also announced "8 Mbps", the number became absurd enough
for someone to redo the math.

The Windows host printed nothing. Today all three print it.

Same story with the cost of the NV12→BGRA conversion: 28 ms per frame,
a ceiling of 35 fps, holding drawing at 27 while the network delivered
60. Nobody had measured it because "it's just a loop". Measuring it cost
one test and pointed straight at it; today it is a budget test that
breaks the build.

**If a protection, a target or a cost doesn't appear anywhere, assume it
is wrong.** The same failure has been seen outside this repository, in a
`dispatch.backfill_gate` flag that existed, was documented, was turned
on, and wasn't read by any code.

## See also

- [[remote-powershell]] — how to send without breaking
- [[restart-the-agent]] — restarting without checking is half the job
