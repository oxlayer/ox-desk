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
# Primeiro SEM declarar DPI: e o que o logistream-host e hoje.
$p = New-Object C+POINT
[C]::GetCursorPos([ref]$p) | Out-Null
$a = "sem-dpi-aware: cursor=$($p.X),$($p.Y) virtual=$([C]::GetSystemMetrics(78))x$([C]::GetSystemMetrics(79))"
# Agora declarando. Mesma maquina, mesmo instante, numeros diferentes
# = a escala esta em jogo e o host esta lendo o espaco errado.
[C]::SetProcessDPIAware() | Out-Null
$q = New-Object C+POINT
[C]::GetCursorPos([ref]$q) | Out-Null
$b = "com-dpi-aware: cursor=$($q.X),$($q.Y) virtual=$([C]::GetSystemMetrics(78))x$([C]::GetSystemMetrics(79))"
"$a`r`n$b" | Set-Content -Path "$env:LOCALAPPDATA\logid\cursor.txt" -Encoding ascii
