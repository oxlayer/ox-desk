# 014 — Windows lock screen (Parsec-style)

> Status: **mechanism proven**, service not written. The measurement with
> the machine actually locked is still missing.

## The problem

The Windows remote screen dies when the session locks. `capture_win.rs`
already documents the four causes of the same `E_ACCESSDENIED`
([rule](../.claude/rules/windows-locked-screen.md)), and three of them
are the same thing underneath: **the input desktop changed**.

The lock screen, the UAC dialog and the screen saver live on their own
desktops inside the `WinSta0` window station. DXGI does not duplicate a
desktop that is not the input desktop, and only `LocalSystem` can attach
to the `Winlogon` desktop.

This matters more once the machine is reached from outside the LAN
([spec 012](012-outside-the-lan.md)): it will then be locked almost all
the time, and today that is exactly the state in which it does not serve
a screen.

## What was measured

With `oxdesk-probe-system`, on the Windows machine of the reference
deployment (Windows 10 Pro, 2560x1440). The probe printed Portuguese;
English translations are added as `#` comments.

**As the user, through the SSH session:**

```
pid 29568 sessão 0                  # session 0
sessão do console: 1                # console session: 1
USERNAME=user (SYSTEM? false)
mesa desta thread: Default          # this thread's desktop: Default
mesa de ENTRADA: não consegui abrir (Função incorreta. 0x80070001)
                                    # INPUT desktop: could not open (Incorrect function.)
```

The whole session boundary in five lines: SSH lands in session 0, the
console is session 1, and the input desktop does not even open
([rule](../.claude/rules/graphical-session.md)).

**As SYSTEM, via a `/ru SYSTEM` task:**

```
winlogon.exe pid 524
privilégio SeDebugPrivilege: ligado                 # privilege SeDebugPrivilege: enabled
privilégio SeIncreaseQuotaPrivilege: ligado
privilégio SeAssignPrimaryTokenPrivilege: ligado
LANCEI pid 11656 na mesa winsta0\Winlogon           # LAUNCHED pid 11656 on desktop winsta0\Winlogon
  → pid 11656 sessão 1                              # session 1
  → USERNAME=WINDOWS-HOST$ (SYSTEM? true)
  → mesa desta thread: Winlogon                     # this thread's desktop
  → mesa de ENTRADA: Default                        # INPUT desktop
  → anexado à mesa de entrada: Default              # attached to the input desktop
  → DUPLICOU: 2560x1440                             # DUPLICATED
```

In other words: the path works. A SYSTEM process launched this way
**switches desktops following the input** and captures.

## The mechanism (the same as Parsec and RustDesk)

1. LocalSystem service, session 0.
2. `WTSGetActiveConsoleSessionId()` → the interactive session.
3. Find `winlogon.exe` **in that session**. It is the target because it
   always exists, runs as SYSTEM, and **keeps existing with no user
   logged in** — which is the lock-screen state. `explorer` will not do:
   it belongs to the user and goes away.
4. `OpenProcessToken` + `DuplicateTokenEx(..., TokenPrimary, ...)`.
   Primary, not impersonation: `CreateProcessAsUser` refuses the other
   kind, and the documentation is explicit.
5. `CreateProcessAsUserW` with `lpDesktop = "winsta0\\Winlogon"`.
6. In the child: `OpenInputDesktop` + `SetThreadDesktop` on every switch,
   and recreate the DXGI duplication.

Privileges: `SeDebugPrivilege` to open winlogon,
`SeIncreaseQuotaPrivilege` and `SeAssignPrimaryTokenPrivilege` for
`CreateProcessAsUser`. Without them the call fails with
`ERROR_PRIVILEGE_NOT_HELD`, and the message does not mention privileges.

A detail that matters: the child is born with
`CREATE_NO_WINDOW`. With `CREATE_NEW_CONSOLE` it would open a black
window **on the lock screen** — right where nobody wants to see one.

## What remains to measure, and why it was not measured

**The locked machine.** With it unlocked the input desktop is `Default`,
and that is what the probe captured. Locked, the desktop becomes
`Winlogon`, and then the behavior **varies by Windows version**: there
are reports of the process dying right after locking on Win 11 and
reports of it working as SYSTEM. There are also reports of DXGI
initializing on Winlogon and returning **empty** frames without failing
— hence the need to fall back to GDI.

This cannot be decided by reading. The test requires actually locking
the machine, and getting back in takes the account password at the
console, so it is left as an explicit step for an operator at the
machine rather than something to run unattended.

```powershell
# lock, wait, run as SYSTEM, read
rundll32 user32.dll,LockWorkStation
schtasks /run /tn oxdesk-probe-system
Get-Content C:\ProgramData\oxdesk\probe-sistema.log
```

## Proposed design

A new binary, not the current host:

| Today | Proposed |
|---|---|
| `/IT` task, runs as the user | LocalSystem service launches the host in the console session |
| captures only the `Default` desktop | follows the input desktop |
| dies on lock | survives |

### The decision that is not obvious

Running the host **always** as SYSTEM is more privilege than it needs
95% of the time, and the reference Windows machine has already had
`oxdesk.exe` deleted by Defender once (spec 008). A permanent SYSTEM binary that captures the
screen and injects keystrokes is exactly the profile an antivirus hates
— with reason.

Alternative: keep the user host as the main one and have the service
take over only when the input desktop stops being `Default`. It costs a
handoff and a second connection; it gains that the common case runs
without privilege.

The choice depends on what the measurement with the machine locked
shows: if the SYSTEM process dies on lock (as one report says), the
handoff becomes mandatory anyway.

## Why a probe before a service

Installing a service touches `HKLM` and creates a permanent SYSTEM
process. The probe achieves the same effect via a `/ru SYSTEM` task,
which the administrator creates and deletes without residue — and it
answers the question that decides the design before asking for the
bigger change. Same spirit as `mac-probe`: prove the pieces before
assembling the pipeline.

## Minor pending items

- The probe log (`C:\ProgramData\oxdesk\probe-sistema.log`) is written in
  UTF-8 and PowerShell reads it as ANSI: accents arrive garbled.
  Cosmetic, and it does not affect the measurement, but it is misleading
  to read.
- `SendSAS` (remote Ctrl+Alt+Del) was not tried. Without it the machine
  cannot be unlocked from outside — only the lock screen can be seen.

## See also

- [[../.claude/rules/windows-locked-screen.md]] — the four causes of the same error
- [[../.claude/rules/graphical-session.md]] — the boundary, measured again here
- [[012-outside-the-lan]] — why this became a priority
