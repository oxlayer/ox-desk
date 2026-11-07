# Sent is not arrived; and a number nobody prints, nobody checks

Two faces of the same rule, both paid for dearly.

## 1. Every remote step checks what arrived

An `scp` that "worked" doesn't guarantee the right file at the
destination. Defender once **restored the quarantined copy on top of**
the freshly sent `logid-agent.exe` — 402984 bytes where 458792 had been
sent. The deploy said ok.

That's why each Windows step in `deploy.sh` captures stderr, tests the
result and checks the size of the file that landed **in PowerShell**,
not in bash (where the expansion would happen on the wrong side).

Same family: `cargo test` does **not** build the deliverable. Running
the tests and installing while thinking the binary had been updated
already cost an hour of confusion about why the validation wasn't
taking effect.

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
is wrong.** See also the `dispatch.backfill_gate` flag in the other
repo: it existed, was documented, was turned on, and wasn't read by any
code.

## See also

- [[remote-powershell]] — how to send without breaking
- [[restart-the-agent]] — restarting without checking is half the job
