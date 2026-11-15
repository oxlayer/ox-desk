# Test the screen host with the screen off

A regression that has already happened once: with the video output
turned off by power saving, the host started, accepted a client and
**never sent a frame**. See [[../rules/windows-locked-screen]].

The case only shows up when the machine has been left alone for a while
— which is exactly when you want to reach it. That's why it's worth
forcing it.

## 1. Install the task that turns off the screen

It must run **in the interactive session**: over SSH it lands in
session 0, where there is no desktop to turn off. Hence the task with
`/it`.

```powershell
# screen-off.ps1, in %LOCALAPPDATA%\oxdesk\
$src = @"
using System;
using System.Runtime.InteropServices;
public class Display {
  [DllImport("user32.dll")]
  public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
}
"@
Add-Type -TypeDefinition $src
# HWND_BROADCAST=0xffff, WM_SYSCOMMAND=0x0112, SC_MONITORPOWER=0xF170, 2=turn off
[Display]::SendMessage([IntPtr]0xffff, 0x0112, [IntPtr]0xF170, [IntPtr]2) | Out-Null
```

```powershell
schtasks /create /tn oxdesk-screen-off /tr "powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$ps`"" /sc once /st 00:00 /it /f
```

## 2. Run the test

```bash
ssh windows-host 'taskkill /F /IM oxdesk-host.exe'      # NOT Stop-Process
sleep 2
ssh windows-host 'powershell -NoProfile -Command "schtasks /run /tn oxdesk-screen-off | Out-Null"'
sleep 25                                              # time for it to really go off
ssh windows-host 'powershell -NoProfile -Command "schtasks /run /tn oxdesk-host | Out-Null"'
sleep 4
cargo run --release --bin oxdesk-sink -- windows-host.local:9777 14
```

## 3. What passing looks like

The `sink` must report frames (dozens in 14 s). And in the host log,
`sending frame 0 KEY` right after `NVENC ready` — **without** any
`DuplicateOutput refused` line and **without** `no frame in Ns`.

If `no frame in 3s; waking the screen` shows up, the watchdog saved the
session but the startup wake-up failed. That is worth investigating,
because the watchdog is a safety net, not the mechanism.

## 4. Clean up

A leftover task that turns off the screen causes confusion later:

```powershell
schtasks /delete /tn oxdesk-screen-off /f
Remove-Item (Join-Path $env:LOCALAPPDATA 'oxdesk\screen-off.ps1')
```
