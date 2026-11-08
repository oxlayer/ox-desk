$src = @"
using System;
using System.Runtime.InteropServices;
public class C {
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public struct POINT { public int X; public int Y; }
}
"@
Add-Type -TypeDefinition $src
# First WITHOUT declaring DPI awareness: that is what oxdesk-host is today.
$p = New-Object C+POINT
[C]::GetCursorPos([ref]$p) | Out-Null
$a = "sem-dpi-aware: cursor=$($p.X),$($p.Y) virtual=$([C]::GetSystemMetrics(78))x$([C]::GetSystemMetrics(79))"
# Now declaring it. Same machine, same instant, different numbers
# = scaling is in play and the host is reading the wrong space.
[C]::SetProcessDPIAware() | Out-Null
$q = New-Object C+POINT
[C]::GetCursorPos([ref]$q) | Out-Null
$b = "com-dpi-aware: cursor=$($q.X),$($q.Y) virtual=$([C]::GetSystemMetrics(78))x$([C]::GetSystemMetrics(79))"
"$a`r`n$b" | Set-Content -Path "$env:LOCALAPPDATA\oxdesk\cursor.txt" -Encoding ascii
