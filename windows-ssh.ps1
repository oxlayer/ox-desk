# Lets this Ubuntu box manage Windows over SSH.
# Run in PowerShell AS ADMINISTRATOR:
#   powershell -ExecutionPolicy Bypass -File windows-ssh.ps1
#
# Installs OpenSSH Server, makes PowerShell the default shell and
# authorizes ubuntu-host's public key.

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal]     [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Run as Administrator.'
}

$pub = 'ssh-ed25519 <your-public-key> -> windows'

Write-Host '==> installing OpenSSH Server (Windows optional feature)'
$cap = Get-WindowsCapability -Online -Name 'OpenSSH.Server*'
if ($cap.State -ne 'Installed') {
  Add-WindowsCapability -Online -Name $cap.Name | Out-Null
} else { Write-Host '  already installed' }

Write-Host '==> sshd service: automatic + started'
Set-Service -Name sshd -StartupType Automatic
Start-Service sshd
Get-Service sshd | Select-Object Name, Status, StartType | Format-Table -AutoSize

Write-Host '==> firewall for port 22'
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue)) {
  New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)'     -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22     -Profile Private,Domain | Out-Null
  Write-Host '  rule created'
} else { Write-Host '  rule already exists' }

Write-Host '==> PowerShell as the default SSH shell'
New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell   -Value (Get-Command powershell.exe).Source -PropertyType String -Force | Out-Null

# GOTCHA: for accounts in the Administrators group, Windows sshd does NOT
# read ~/.ssh/authorized_keys -- it reads this file, and requires a
# restricted ACL.
Write-Host '==> authorizing the key (administrators_authorized_keys)'
$akf = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys'
if (Test-Path $akf) {
  $cur = Get-Content $akf -Raw -ErrorAction SilentlyContinue
} else { $cur = '' }
if ($cur -notlike "*$pub*") {
  Add-Content -Path $akf -Value $pub -Encoding ascii
  Write-Host '  key added'
} else { Write-Host '  key already authorized' }

Write-Host '==> fixing the ACL (without it sshd silently ignores the file)'
icacls $akf /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null
icacls $akf

Restart-Service sshd
Write-Host ''
Write-Host 'Done. Test from Ubuntu:'
Write-Host "  ssh -i ~/.ssh/id_ed25519_windows $env:USERNAME@192.168.1.43 'hostname'"
Write-Host ''
Write-Host 'Undo:  Stop-Service sshd; Set-Service sshd -StartupType Disabled'
