# Lets the machine you drive from manage this Windows box over SSH.
# Run in PowerShell AS ADMINISTRATOR, passing the public key of that
# machine:
#   powershell -ExecutionPolicy Bypass -File windows-ssh.ps1 -PublicKey 'ssh-ed25519 AAAA...'
#
# Installs OpenSSH Server, makes PowerShell the default shell and
# authorizes the key you passed.
param([string]$PublicKey)

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal]     [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Run as Administrator.'
}

# The key to authorize comes from whoever runs this, never from the repo:
# a literal here authorized the MAINTAINER's key on the machine of anyone
# who followed the README. See .claude/rules/no-secrets-in-source.md.
$pub = $PublicKey
if (-not $pub) {
  throw "usage: .\windows-ssh.ps1 -PublicKey '<ssh public key>'  (the key of the machine you drive from)"
}
if ($pub -notmatch '^(ssh-ed25519|ssh-rsa|ecdsa-sha2-\S+|sk-ssh-\S+) ') {
  throw "that does not look like an SSH public key"
}

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
