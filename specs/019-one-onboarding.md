# 019 — One onboarding, not a pile of scripts

> Written for: whoever consolidates the setup paths, and whoever has to
> tell them where the floor is.

Today a new machine is prepared by shell and PowerShell scripts, one set
per operating system, pasted from a README by whoever remembers the order.
The target is a single onboarding driven by the panel, where each step is
implemented against the operating system's own API and the scripts that
survive are the ones that genuinely cannot be anything else.

This spec says what exists, what can move, and — the part that decides the
design — what cannot move no matter how the code is arranged.

## What exists today, measured (2026-11-08)

**26 shell and PowerShell files, 2,674 lines.** They are not one pile;
they are two, and only one of them is the problem:

| Kind | Lines | Examples | Who runs it |
|---|---:|---|---|
| Maintainer tooling | ~1,540 | `deploy.sh` (640), `build-binaries.sh` (283), `check.sh`, `check-overlay.sh`, `sign-windows.sh`, `migrate-from-logi/*` (389) | us, on our machine |
| User onboarding | ~1,100 | `install.sh`, `status.sh`, `windows-setup.ps1`, `windows-ssh.ps1`, `enable-mac.sh`, `enable-sshd.sh`, `enable-ufw.sh`, `install-tailscale*.sh`, `install-virtual-display.ps1`, `mac/create-identity.sh`, `mac/sign.sh` | a person setting up a machine |

The maintainer half stays shell and that is fine: a contributor never runs
`deploy.sh` to install the product, and a release pipeline written in Rust
would be worse, not better. **The user half is the subject of this spec.**

**And the wizard already exists.** `oxdesk/src/setup*.rs` is 2,831 lines,
and what it shows a person is **24 items** — not the "126 steps" an
earlier draft of this spec claimed, which was a count of `Check::`
constructions in the source and not of anything a user sees. Corrected,
by label:

| | Items | Which |
|---|---:|---|
| Linux | 6 | `wayland`, `receptor`, `input-kvm`, `nvenc`, `binarios`, `daemon` |
| **macOS** | **9** | `ffmpeg`, `identidade`, `assinatura`, `gravacao-de-tela`, `acessibilidade`, `launchagent`, `tailscale`, `binarios`, `daemon` |
| Windows | 5 | `sshd`, `chave-admin`, `tela-virtual`, `protetor-de-tela`, `binarios` |
| Shared | 4 | |

The daemon even carries two scripts inside the binary with `include_str!`
and writes them out when a step needs them
(`install-virtual-display.ps1`, `install-tailscale-mac.sh`).

### The Mac is the worst of the three, and that is readable in the table

Nine items against five and six, and the extra ones are not arbitrary:

| Item | Why it exists | Can it go? |
|---|---|---|
| `ffmpeg` | The only platform that links it, for capture and encode | **Yes** — both are Apple frameworks underneath. [[020-native-mac]] |
| `identidade` + `assinatura` | A local signing identity keeps the binary's designated requirement stable, so a rebuild does not lose the TCC grant | No, but they are **one** concern and should be one item |
| `gravacao-de-tela` + `acessibilidade` | TCC grants, which Apple does not let software give itself | No. Present them as "do this in Settings, I will re-check" and never claim to have fixed them |
| `launchagent`, `binarios`, `daemon` | Same as the other platforms | No |
| `tailscale` | Optional, for reaching the machine outside the LAN | Should be behind "do you want this?", not in the main list |

So nine becomes six: drop `ffmpeg`, merge the signing pair, and move
`tailscale` out of the default path. The two TCC items stay, and the
wizard's job there is to be honest rather than clever.

So this is **consolidation, not greenfield**. The question is not "should
there be a wizard" — there is one. It is "why does a person still paste
`enable-sshd.sh` into a terminal when the wizard has 126 steps".

## The floor: what cannot move into the GUI

Three things, and no amount of architecture removes them. Any plan that
pretends otherwise will produce a wizard that lies about what it did.

### 1. Elevation

`Add-WindowsCapability -Online` for OpenSSH Server, firewall rules,
`schtasks /create` under another principal, `udev` rules for raw HID
access, `ufw`. A GUI can *request* elevation — UAC on Windows,
Authorization Services on macOS, polkit on Linux — but the user still
approves it, and what runs with those rights is the same command. Packaging
it in a dialog does not make it not-a-privileged-operation.

What this buys even so: **one prompt instead of five**, the command shown
before it runs, and the result verified afterwards. That is a real
improvement over pasting, and it is honest about what it is.

### 2. macOS TCC grants

Screen Recording and Accessibility cannot be granted programmatically.
That is Apple's design, not a missing API, and it is why the grant follows
the *binary* and is lost on a rename. The wizard's only possible role is
to detect the state, open the right Settings pane, and re-check — never to
claim it fixed something.

### 3. Third-party package managers

Homebrew's `ffmpeg@7`, Tailscale, OpenSSH. The wizard can drive them and
read their exit codes, but the work is still "ask a package manager", and
their failures are theirs. Measured on 2026-11-08: `brew install ffmpeg@7`
on macOS 14 fails outright, because that macOS version is Tier 3 for
Homebrew and has no bottle — a wizard step must be able to report *that*
instead of hanging on a source build.

## The shape to build

One state machine, one list of steps, per-OS implementations behind a
trait. Every step answers four questions, in this order:

| | |
|---|---|
| **Detect** | What is the state right now? Never "did we run the script". |
| **Explain** | In one sentence a person can act on, naming the thing that is missing. |
| **Remediate** | Only if it can be done honestly. Otherwise say who must do it. |
| **Verify** | Re-detect. A step that cannot fail is decoration ([[../.claude/rules/verify-what-arrived]]). |

Consequences worth stating, because each one kills a shortcut:

- **Idempotent by detection, not by flag.** No "already configured" file.
  The question is always the live state, because the user may have undone
  it by hand.
- **A step that needs elevation says so before asking**, shows the exact
  command, and runs it through the platform's own elevation — not a
  `sudo` inside a script the user pasted.
- **A step that cannot remediate is not a failure**, it is a `blocked`
  with instructions. The TCC steps are the archetype.
- **The scripts that survive live inside the binary**, written out on
  demand like the two that already do. A script the daemon carries is
  versioned with the daemon; a script in a README is versioned by
  whoever last copied it.
- **The panel and the API show the same list.** The wizard is an API
  (`/setup`) with a UI on top, so onboarding over SSH on a headless
  machine remains possible.

## Adversarial review

### "This is a rewrite of 1,100 working lines for aesthetics"

Largely true today. The scripts work; the person who wrote them knows the
order. The cost is paid by the *second* user, and until recently there was
no second user.

What survives: going open source makes the second user real, and the
failure mode is not aesthetic. `enable-mac.sh` carried a literal SSH
public key and authorized it on the machine that ran it
([[018-open-source]]) — a defect that exists *because* onboarding is a pile
of scripts nobody reviews as a product surface. Consolidation is how that
class of defect stops being possible.

### "A GUI wizard hides what is happening, which is worse for a security tool"

A real risk. A terminal shows the command; a button does not.

What survives: the mitigation is in the shape above — show the command
before running it, show what was verified after, and never claim a step
succeeded without re-detecting. If a step cannot be shown and verified, it
should stay a documented command the user runs themselves.

### "Per-OS APIs mean three implementations instead of one script each"

Also true: a trait with three implementations is not less code than three
scripts, and it is harder to read for someone who only has one of the
three operating systems.

What survives: the win is not line count, it is that the *list of steps*
and their order exist once, in one place, tested. The platform specifics
were always three implementations; today they are three implementations
plus three copies of the order, and the copies drift.

### "Elevation and TCC mean it can never be one click"

Correct, and it is why this spec starts with the floor. The promise is not
one click; it is **one list, one place, honest about what it cannot do**.

## Order of work

1. **Inventory the user-facing scripts against the 126 wizard steps** and
   mark each line of script as: already covered, coverable, or floor.
   *Check:* a table in this spec with no "unknown" rows.
2. **Define the step trait** and port the steps that are already pure
   detection. *Check:* the wizard's output is unchanged on all three
   machines.
3. **Move elevation behind one platform-specific prompt** per OS.
   *Check:* a fresh machine reaches "ready" with exactly one elevation
   prompt per OS.
4. **Delete the scripts whose steps are covered**, and move the survivors
   inside the binary. *Check:* `git ls-files '*.sh' '*.ps1'` drops to the
   maintainer half plus the floor.
5. **Make `status.sh` an API call.** It reads state and prints it; that is
   the panel's job already.

## See also

- [[011-onboarding]] — the wizard's current design
- [[018-open-source]] — why the second user became real, and the SSH key
  this pile was hiding
- [[../.claude/rules/verify-what-arrived]] — a step that cannot fail is
  not a check
