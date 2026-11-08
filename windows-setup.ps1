# Windows side (192.168.1.43) of the Flow-equivalent setup.
# Run in PowerShell AS ADMINISTRATOR:
#   powershell -ExecutionPolicy Bypass -File windows-setup.ps1

$ErrorActionPreference = 'Stop'
$ver = '1.27.0'
$msi = "$env:TEMP\deskflow-$ver-win-x64.msi"
$url = "https://github.com/deskflow/deskflow/releases/download/v$ver/deskflow-$ver-win-x64.msi"

if (-not ([Security.Principal.WindowsPrincipal] `
    [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Run this script as Administrator.'
}

Write-Host "==> downloading deskflow $ver"
Invoke-WebRequest -Uri $url -OutFile $msi

Write-Host '==> installing (silent)'
Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn', '/norestart' -Wait

Write-Host '==> firewall rule for port 24800 (deskflow server)'
Get-NetFirewallRule -DisplayName 'Deskflow' -ErrorAction SilentlyContinue |
  Remove-NetFirewallRule
New-NetFirewallRule -DisplayName 'Deskflow' -Direction Inbound -Action Allow `
  -Protocol TCP -LocalPort 24800 -Profile Private,Domain | Out-Null

Write-Host '==> network profile (the rule above only applies on a Private network)'
Get-NetConnectionProfile | Select-Object InterfaceAlias, NetworkCategory |
  Format-Table -AutoSize
Write-Host 'If "Public" shows above, switch it to Private:'
Write-Host '  Set-NetConnectionProfile -InterfaceAlias "<name>" -NetworkCategory Private'

Write-Host '==> autostart at logon (shortcut in the Startup folder)'
# The server needs a user session to capture input, so it runs at LOGON
# -- not as a "whether user is logged on or not" service.
$exe = Join-Path ${env:ProgramFiles} 'Deskflow\deskflow.exe'
if (-not (Test-Path $exe)) {
  $exe = (Get-ChildItem -Path "${env:ProgramFiles}","${env:ProgramFiles(x86)}" `
          -Filter 'deskflow.exe' -Recurse -ErrorAction SilentlyContinue |
          Select-Object -First 1).FullName
}
if ($exe) {
  $startup = [Environment]::GetFolderPath('Startup')
  $lnk = Join-Path $startup 'Deskflow.lnk'
  $ws = New-Object -ComObject WScript.Shell
  $sc = $ws.CreateShortcut($lnk)
  $sc.TargetPath = $exe
  $sc.WorkingDirectory = Split-Path $exe
  $sc.Description = 'Deskflow server (KVM over LAN)'
  $sc.Save()
  Write-Host "  shortcut created: $lnk"
  Write-Host '  confirm in the GUI that the saved mode is Server and that it starts the core.'
} else {
  Write-Host '  deskflow.exe not found -- create the shortcut by hand:'
  Write-Host '  Win+R -> shell:startup -> drag a Deskflow shortcut there'
}

Write-Host ''
Write-Host 'Done. Now:'
Write-Host '  1. open Deskflow, choose Server'
Write-Host '  2. Configure Server -> drag a new screen next to Windows'
Write-Host '     (the EXACT hostname the Ubuntu client shows)'
Write-Host '  3. install KDE Connect from the Microsoft Store (clipboard + files)'

# --- oxdesk-stream: screen and input ---------------------------------------
#
# Two ports, two rules. The input one (9778) was missing when LAN KVM
# arrived, and the symptom did not say "firewall": the KVM grabbed the
# keyboard here, did not connect there, and the keyboard came back on its
# own. A separate rule on purpose -- whoever does not want the KVM
# disables only that one. The rule display names are left as they are:
# they are how this script finds an existing rule.
foreach ($r in @(
  @{ nome = 'oxdesk-stream (tela remota) UDP 9777'; porta = 9777 },
  @{ nome = 'oxdesk-stream (input KVM) UDP 9778';   porta = 9778 }
)) {
  if (-not (Get-NetFirewallRule -DisplayName $r.nome -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $r.nome -Direction Inbound -Protocol UDP `
      -LocalPort $r.porta -Action Allow -Profile Any | Out-Null
    Write-Host ("rule created: " + $r.nome)
  } else {
    Write-Host ("rule already exists: " + $r.nome)
  }
}
