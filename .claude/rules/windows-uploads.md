# Uploading to Windows: one cause can look like three failures

Six steps in `deploy.sh` send a signed `.exe` to the Windows machine as
`<name>.new.exe`, swap it over the live file and delete the staging copy.
This is the trap that family of steps keeps falling into, and what is and
is not known about it.

## The symptom

```
  windows: screen host (oxdesk-stream)        FAILED
      Overwriting an existing file is not supported.
```

**The message names no file.** That single fact is what made one cause
look like three unrelated failures: across four runs it was reported by
`agent`, by `screen viewer`, by `tab bridge` and by `screen host`,
whichever happened to hit it, and each time the obvious move was to
investigate that step.

## What is established

- **An interrupted deploy leaves a partial staging file.** Killing a run
  mid-upload left `oxdesk-bridge.new.exe` at 6,005,760 bytes against the
  complete 10,220,048. So the leftover is real and the mechanism that
  creates it is known.
- **Overwriting is NOT the problem**, despite what the message says and
  what this repo's own comment claimed for weeks. Measured by hand:
  `scp` over a complete file of the same size succeeds; `scp` of a
  complete 10 MB file over a **partial** 6 MB one also succeeds, exit 0.
  Both were tested directly against the live machine.
- **The cleanup works.** Running the removal's PowerShell by itself:
  before, `oxdesk-bridge.new.exe`; after, nothing; no error.
- **The whole step works by hand.** Reproducing it exactly — `cp`,
  `sign-windows.sh`, `scp` — exits 0 every time.
- **The updater is innocent.** `ota/src/apply.rs` copies straight to
  `install_dir/<name>` and never writes a `.new.exe`, so there is no race
  with its scheduled task.

## What is not established

**Why it happens.** It appears only inside a full deploy, only in later
steps, and it is intermittent — four runs failed, the fifth passed with no
relevant change. Three hypotheses were raised and each one was killed by
measurement: leftover from a previous run, a cleanup that did not run, and
a server that refuses to truncate. Do not re-litigate those without new
evidence.

## What the code does about it now

- Every upload goes through `send()`, which clears the staging path
  **immediately before its own `scp`** rather than once at the top of the
  section. Cleaning at the top was tried and only moved the failure to a
  later step, which is itself evidence that whatever leaves the file does
  it *during* the run.
- When an upload fails, `send()` asks the machine what is there — the
  most recent `.exe` files with size and time, and the running `oxdesk*`
  processes — and prints it with the error. The next occurrence produces
  evidence instead of a sentence with no filename.

## Two rules for whoever picks this up

1. **Do not change the working tree while a deploy is running.** Switching
   branches mid-run is how the partial file above was created: the deploy
   reads source from disk as it goes, so the fleet ends up on a mix and
   the final check still passes, because the build stamp comes from the
   environment and not from the code.
2. **Measure before writing the cause into a comment.** The claim that the
   SFTP server refuses to overwrite sat in `deploy.sh` long enough to be
   quoted as fact, including during this investigation. It cost an
   afternoon of chasing the wrong thing.

## See also

- [[verify-what-arrived]] — including why a pipe ate three exit codes in
  one day
- [[remote-powershell]] — the transport these steps use
