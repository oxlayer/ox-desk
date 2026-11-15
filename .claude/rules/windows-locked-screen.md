# `E_ACCESSDENIED` in DXGI: four causes, and the most common is the screensaver

`DuplicateOutput` returns `E_ACCESSDENIED (0x80070005)` when the process
**is not on the current input desktop**. Windows has several desktops
per session, and DXGI only duplicates for whoever is on the one
receiving input.

## The screensaver is cause number one, and it cost three diagnoses

On 2026-11-04 the Windows machine's screen could not be reached. The
first diagnosis was "locked screen", the second "half-installed video
driver". Measurement ruled out both: `LogonUI` **absent**, RTX 3050 with
status OK and `cfg=0`, and still refused.

It was `Bubbles.scr` running. The screensaver lives on its own desktop
("Screen-saver"), which becomes the input desktop — and our process, on
`Default`, loses access.

What found it was **listing processes** instead of testing one more
hypothesis:

```powershell
Get-Process | Where-Object { $_.Path -like '*.scr' }
```

Two traps that cost time:

- `SendInput` does **not** dismiss the screensaver. The injection goes
  to the desktop of the calling thread (`Default`); the screensaver, on
  another desktop, never hears of it. That's why `wake_screen()` solves
  a screen that is off and doesn't solve this one — two problems that
  looked like one.
- `SPI_GETSCREENSAVERRUNNING` queried over SSH returns `False` even with
  the screensaver up, because session 0 can't see session 1's desktop.
  The process list can.

Today the host kills any `*.scr` at startup
(`end_screensaver()`), which is what remote access tools do. A
screensaver has no state to lose.

## Diagnosis: four causes, same error code

In order of measured frequency.

```powershell
# 1. screensaver running?  (the most common cause)
Get-Process | Where-Object { $_.Path -like '*.scr' }

# 2. lock screen / UAC in front?
Get-Process -Name LogonUI -ErrorAction SilentlyContinue   # present = locked
Get-Process -Name consent -ErrorAction SilentlyContinue   # present = UAC

# 3. half-installed video stack?
Get-CimInstance Win32_VideoController |
  ForEach-Object { $_.Name + " [" + $_.Status + "] cfg=" + $_.ConfigManagerErrorCode }
# a "Microsoft Basic Display Adapter" with status != OK = half-installed driver

# 4. pending reboot?
Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending'
```

The half-installed driver was real in the morning (Parsec's virtual
adapter was removed and left a `Microsoft Basic Display Adapter` in
`Unknown`) and then resolved itself. In the afternoon the adapter was OK
and the cause was the screensaver. **Same error, different cause, on the
same day** — that's why the checklist exists, and why it starts with the
process list instead of the favorite hypothesis.

## A screen that is OFF is something else, and that one we solved

Not to be confused with power saving having turned off the video output.
There `DuplicateOutput` **passes**, and the symptom is harder to find:
DXGI returns `CaptureState::Idle` forever, there is never a first frame, and
the client sits on black without anyone complaining.

Solved in `host_win.rs`: `wake_screen()` at startup (a
`MOUSEEVENTF_MOVE` of +1 and -1 in the same call — one of (0,0) is a
no-op and Windows discards it), `ScreenAwake` holding `ES_DISPLAY_REQUIRED`
while we serve, and a watchdog that nudges at 3 s and recreates the
duplication at 9 s if no frame shows up.

## What it would take to capture the lock screen

A service running as SYSTEM in session 0 that discovers the active
console session (`WTSGetActiveConsoleSessionId`), takes the token
(`WTSQueryUserToken`) and launches the capturer with
`CreateProcessAsUser`, pointing `lpDesktop` to the current desktop —
`Winlogon` when locked, `Default` when not. And relaunches it when the
desktop changes.

It's what Parsec and AnyDesk do. It's not built.

## See also

- [[../../specs/010-remote-desktop.md]] — the whole screen stack
- [[verify-what-arrived]] — why the host log is the source, not the client
