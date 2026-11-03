# Cria o atalho de autostart do Deskflow no Windows -- sem reinstalar nada.
# Rode no PowerShell (NAO precisa ser admin):
#   powershell -ExecutionPolicy Bypass -File windows-autostart.ps1
#
# Logon, nao servico: o servidor precisa de uma sessao de usuario para
# capturar teclado e mouse, entao "run whether user is logged on or not"
# nao serve.
#
# Flags:
#   -Remove   apaga o atalho em vez de criar

param([switch]$Remove)

$ErrorActionPreference = 'Stop'
$startup = [Environment]::GetFolderPath('Startup')
$lnk     = Join-Path $startup 'Deskflow.lnk'

if ($Remove) {
  if (Test-Path $lnk) { Remove-Item $lnk -Force; Write-Host "removido: $lnk" }
  else { Write-Host 'nada para remover.' }
  return
}

Write-Host '==> procurando deskflow.exe'
$candidates = @(
  (Join-Path ${env:ProgramFiles} 'Deskflow\deskflow.exe'),
  (Join-Path ${env:ProgramFiles(x86)} 'Deskflow\deskflow.exe')
) | Where-Object { $_ -and (Test-Path $_) }

if (-not $candidates) {
  $roots = @(${env:ProgramFiles}, ${env:ProgramFiles(x86)}, ${env:LOCALAPPDATA}) |
           Where-Object { $_ -and (Test-Path $_) }
  $candidates = (Get-ChildItem -Path $roots -Filter 'deskflow.exe' -Recurse `
                   -ErrorAction SilentlyContinue |
                 Select-Object -First 1).FullName
}

$exe = $candidates | Select-Object -First 1
if (-not $exe) {
  Write-Host ''
  Write-Host 'deskflow.exe nao encontrado. O Deskflow esta instalado?'
  Write-Host 'Instale com o windows-setup.ps1, ou crie o atalho na mao:'
  Write-Host '  Win+R -> shell:startup -> arraste um atalho do Deskflow para la'
  exit 1
}
Write-Host "  encontrado: $exe"

Write-Host '==> criando atalho'
$ws = New-Object -ComObject WScript.Shell
$sc = $ws.CreateShortcut($lnk)
$sc.TargetPath       = $exe
$sc.WorkingDirectory = Split-Path $exe
$sc.Description      = 'Deskflow server (KVM over LAN)'
$sc.Save()

if (Test-Path $lnk) {
  Write-Host "  ok: $lnk"
} else {
  Write-Host '  FALHOU: atalho nao foi criado'; exit 1
}

Write-Host ''
Write-Host 'Confira na GUI do Deskflow, uma vez:'
Write-Host '  - modo salvo = Server'
Write-Host '  - a grade tem a tela ubuntu-host no lado fisico correto'
Write-Host '  - a GUI inicia o core sozinha ao abrir'
Write-Host ''
Write-Host 'Testar sem reiniciar:  Win+R -> shell:startup  (clique no atalho)'
Write-Host 'Desfazer:              -Remove'
