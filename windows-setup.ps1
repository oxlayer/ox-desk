# Lado Windows (192.168.1.43) do setup Flow-equivalente.
# Rode no PowerShell COMO ADMINISTRADOR:
#   powershell -ExecutionPolicy Bypass -File windows-setup.ps1

$ErrorActionPreference = 'Stop'
$ver = '1.27.0'
$msi = "$env:TEMP\deskflow-$ver-win-x64.msi"
$url = "https://github.com/deskflow/deskflow/releases/download/v$ver/deskflow-$ver-win-x64.msi"

if (-not ([Security.Principal.WindowsPrincipal] `
    [Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Rode este script como Administrador.'
}

Write-Host "==> baixando deskflow $ver"
Invoke-WebRequest -Uri $url -OutFile $msi

Write-Host '==> instalando (silencioso)'
Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn', '/norestart' -Wait

Write-Host '==> regra de firewall para a porta 24800 (servidor deskflow)'
Get-NetFirewallRule -DisplayName 'Deskflow' -ErrorAction SilentlyContinue |
  Remove-NetFirewallRule
New-NetFirewallRule -DisplayName 'Deskflow' -Direction Inbound -Action Allow `
  -Protocol TCP -LocalPort 24800 -Profile Private,Domain | Out-Null

Write-Host '==> perfil da rede (a regra acima so vale em rede Privada)'
Get-NetConnectionProfile | Select-Object InterfaceAlias, NetworkCategory |
  Format-Table -AutoSize
Write-Host 'Se aparecer "Public" acima, troque para Private:'
Write-Host '  Set-NetConnectionProfile -InterfaceAlias "<nome>" -NetworkCategory Private'

Write-Host '==> autostart no logon (atalho na pasta Startup)'
# O servidor precisa de uma sessao de usuario para capturar input, entao
# roda no LOGON -- nao como servico "whether user is logged on or not".
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
  Write-Host "  atalho criado: $lnk"
  Write-Host '  confirme na GUI que o modo salvo e Server e que ela inicia o core.'
} else {
  Write-Host '  deskflow.exe nao encontrado -- crie o atalho manualmente:'
  Write-Host '  Win+R -> shell:startup -> arraste um atalho do Deskflow para la'
}

Write-Host ''
Write-Host 'Pronto. Agora:'
Write-Host '  1. abra o Deskflow, escolha Server'
Write-Host '  2. Configure Server -> arraste uma tela nova ao lado do Windows'
Write-Host '     (nome EXATO do hostname que o cliente Ubuntu mostrar)'
Write-Host '  3. instale o KDE Connect pela Microsoft Store (clipboard + arquivos)'
