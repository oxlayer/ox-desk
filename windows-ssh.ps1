# Libera o Windows para ser gerenciado deste Ubuntu via SSH.
# Rode no PowerShell COMO ADMINISTRADOR:
#   powershell -ExecutionPolicy Bypass -File windows-ssh.ps1
#
# Instala o OpenSSH Server, poe o PowerShell como shell default e
# autoriza a chave publica do ubuntu-host.

$ErrorActionPreference = 'Stop'

if (-not ([Security.Principal.WindowsPrincipal]     [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Rode como Administrador.'
}

$pub = 'ssh-ed25519 <your-public-key> -> windows'

Write-Host '==> instalando OpenSSH Server (recurso opcional do Windows)'
$cap = Get-WindowsCapability -Online -Name 'OpenSSH.Server*'
if ($cap.State -ne 'Installed') {
  Add-WindowsCapability -Online -Name $cap.Name | Out-Null
} else { Write-Host '  ja instalado' }

Write-Host '==> servico sshd: automatico + iniciado'
Set-Service -Name sshd -StartupType Automatic
Start-Service sshd
Get-Service sshd | Select-Object Name, Status, StartType | Format-Table -AutoSize

Write-Host '==> firewall para a porta 22'
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue)) {
  New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)'     -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22     -Profile Private,Domain | Out-Null
  Write-Host '  regra criada'
} else { Write-Host '  regra ja existe' }

Write-Host '==> PowerShell como shell default do SSH'
New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell   -Value (Get-Command powershell.exe).Source -PropertyType String -Force | Out-Null

# PEGADINHA: para contas no grupo Administrators, o sshd do Windows NAO le
# ~/.ssh/authorized_keys -- le este arquivo, e exige ACL restrita.
Write-Host '==> autorizando a chave (administrators_authorized_keys)'
$akf = Join-Path $env:ProgramData 'ssh\administrators_authorized_keys'
if (Test-Path $akf) {
  $cur = Get-Content $akf -Raw -ErrorAction SilentlyContinue
} else { $cur = '' }
if ($cur -notlike "*$pub*") {
  Add-Content -Path $akf -Value $pub -Encoding ascii
  Write-Host '  chave adicionada'
} else { Write-Host '  chave ja autorizada' }

Write-Host '==> corrigindo ACL (sem isso o sshd ignora o arquivo em silencio)'
icacls $akf /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null
icacls $akf

Restart-Service sshd
Write-Host ''
Write-Host 'Pronto. Teste do Ubuntu:'
Write-Host "  ssh -i ~/.ssh/id_ed25519_windows $env:USERNAME@192.168.1.43 'hostname'"
Write-Host ''
Write-Host 'Desfazer:  Stop-Service sshd; Set-Service sshd -StartupType Disabled'
