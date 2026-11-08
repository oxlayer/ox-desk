# One-time move of a Windows installation from logi/logid/logistream to
# oxdesk. Spec: specs/017-product.md ("The name: oxdesk").
#
# Run in the user's account (over SSH is fine), BEFORE the first oxdesk
# deploy. Pass -Cleanup after the deploy to remove the old folder, tasks
# and Defender exclusion.
#
# Order matters on this machine: Defender deleted our unsigned binaries the
# instant they were written (specs/008), so the exclusion for the NEW folder
# exists before anything lands in it.
param([switch]$Cleanup)

$old = Join-Path $env:LOCALAPPDATA 'logid'
$new = Join-Path $env:LOCALAPPDATA 'oxdesk'
$tasks = @{ 'logid' = 'oxdesk'; 'logistream-host' = 'oxdesk-host'; 'logi-ota' = 'oxdesk-ota' }
function Say($m) { Write-Output ('  ' + $m) }

if ($Cleanup) {
  $alive = Get-Process -Name oxdesk -ErrorAction SilentlyContinue
  if (-not $alive) { Write-Output 'oxdesk is not running; not cleaning up'; exit 1 }
  foreach ($o in $tasks.Keys) {
    schtasks /query /tn $o 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { schtasks /delete /tn $o /f | Out-Null; Say ('deleted task ' + $o) }
  }
  if (Test-Path $old) { Remove-Item -Recurse -Force $old; Say ('removed ' + $old) }
  if ((Get-MpPreference).ExclusionPath -contains $old) { Remove-MpPreference -ExclusionPath $old; Say 'removed the old Defender exclusion' }
  exit 0
}

Write-Output '== Defender exclusion for the new folder, first'
if (-not ((Get-MpPreference).ExclusionPath -contains $new)) { Add-MpPreference -ExclusionPath $new }
if (-not ((Get-MpPreference).ExclusionPath -contains $new)) { Write-Output 'ERROR: the exclusion did not stick; stopping before any binary lands'; exit 1 }
Say ('excluded ' + $new)

Write-Output '== stop the old tasks and processes'
foreach ($o in $tasks.Keys) { schtasks /end /tn $o 2>$null | Out-Null }
foreach ($p in 'logid.exe','logistream-host.exe','logi-ota.exe','logid-agent.exe') {
  taskkill /F /IM $p 2>$null | Out-Null
  if ($LASTEXITCODE -eq 0) { Say ('stopped ' + $p) }
}
# The panel window: the Electron that lives under the old folder.
Get-Process -Name electron -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$old*" } | ForEach-Object { taskkill /F /PID $_.Id | Out-Null; Say 'closed the panel window' }
Start-Sleep -Seconds 1

Write-Output '== state to the new folder (not the old binaries or logs)'
New-Item -ItemType Directory -Force $new | Out-Null
robocopy $old $new /E /XF *.exe *.log *.log.1 /NFL /NDL /NJH /NJS /NP | Out-Null
if ($LASTEXITCODE -ge 8) { Write-Output ('ERROR: robocopy ' + $LASTEXITCODE); exit 1 }
foreach ($f in 'ota-token','slot-names.json') {
  if ((Test-Path (Join-Path $old $f)) -and -not (Test-Path (Join-Path $new $f))) { Write-Output ('ERROR: ' + $f + ' did not reach the new folder'); exit 1 }
}
$bat = Join-Path $new 'run-gui.bat'
if (Test-Path $bat) { (Get-Content $bat) -replace '\\logid\\','\oxdesk\' | Set-Content $bat; Say 'run-gui.bat points to the new folder' }
Say ('copied to ' + $new)

Write-Output '== config and identity (same moves as oxdesk/src/legacy.rs)'
$cfgOld = Join-Path $env:APPDATA 'logi'; $cfgNew = Join-Path $env:APPDATA 'oxdesk'
if ((Test-Path $cfgOld) -and -not (Test-Path $cfgNew)) { Move-Item $cfgOld $cfgNew; Say ('moved ' + $cfgOld) }
$id = Join-Path $cfgNew 'identity'
if ((Test-Path (Join-Path $cfgNew 'identidade')) -and -not (Test-Path $id)) { Move-Item (Join-Path $cfgNew 'identidade') $id; Say 'identity dir' }
foreach ($pair in @(@('identidade.der','identity.der'), @('autorizados','authorized'))) {
  $a = Join-Path $id $pair[0]; $b = Join-Path $id $pair[1]
  if ((Test-Path $a) -and -not (Test-Path $b)) { Move-Item $a $b; Say $pair[1] }
}

Write-Output '== scheduled tasks: same definition, new names'
foreach ($o in $tasks.Keys) {
  $n = $tasks[$o]
  schtasks /query /tn $n 2>$null | Out-Null
  if ($LASTEXITCODE -eq 0) { Say ($n + ' already exists'); continue }
  $xml = (schtasks /query /tn $o /xml ONE) -join "`r`n"
  if ($LASTEXITCODE -ne 0) { Say ('no task ' + $o); continue }
  $xml = $xml.Replace('\logid\', '\oxdesk\').Replace('logid.exe', 'oxdesk.exe')
  $xml = $xml.Replace('logistream-host.exe', 'oxdesk-host.exe').Replace('logi-ota.exe', 'oxdesk-ota.exe')
  $xml = $xml -replace ('<URI>\\' + [regex]::Escape($o) + '</URI>'), ('<URI>\' + $n + '</URI>')
  $file = Join-Path $env:TEMP ($n + '.xml')
  [System.IO.File]::WriteAllText($file, $xml, [System.Text.Encoding]::Unicode)
  $r = schtasks /create /tn $n /xml $file /f 2>&1
  Remove-Item $file
  if ($LASTEXITCODE -ne 0) { Write-Output ('ERROR: task ' + $n + ': ' + $r); exit 1 }
  schtasks /change /tn $o /disable | Out-Null
  Say ('task ' + $n + ' (old ' + $o + ' disabled, deleted by -Cleanup)')
}

Write-Output '== panel shortcut at logon'
$startup = [Environment]::GetFolderPath('Startup')
$lnkOld = Join-Path $startup 'Logi.lnk'; $lnkNew = Join-Path $startup 'Oxdesk.lnk'
if ((Test-Path $lnkOld) -and -not (Test-Path $lnkNew)) {
  $sh = New-Object -ComObject WScript.Shell
  $o = $sh.CreateShortcut($lnkOld); $n = $sh.CreateShortcut($lnkNew)
  $n.TargetPath = $o.TargetPath.Replace('\logid\', '\oxdesk\')
  $n.Arguments = $o.Arguments.Replace('\logid\', '\oxdesk\')
  $n.WorkingDirectory = $o.WorkingDirectory.Replace('\logid\', '\oxdesk\')
  $n.IconLocation = $o.IconLocation.Replace('\logid\', '\oxdesk\')
  $n.WindowStyle = $o.WindowStyle
  $n.Save(); Remove-Item $lnkOld; Say ('shortcut -> ' + $n.TargetPath)
}

Write-Output '== firewall rule names'
foreach ($pair in @(@('logistream (tela remota) UDP 9777','oxdesk-stream (tela remota) UDP 9777'), @('logistream (input KVM) UDP 9778','oxdesk-stream (input KVM) UDP 9778'))) {
  $r = Get-NetFirewallRule -DisplayName $pair[0] -ErrorAction SilentlyContinue
  if ($r) { $r | Set-NetFirewallRule -NewDisplayName $pair[1]; Say $pair[1] }
}
exit 0
