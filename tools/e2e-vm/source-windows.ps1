# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later
<#
The Windows SOURCE VM for the end-to-end tests (specs/026): Windows 11 with
automatic login, the machine OxDesk sends keyboard and mouse AWAY from.

  tools\e2e-vm\source-windows.ps1 build -Iso <Windows 11 x64 ISO>   # unattended install (once)
  tools\e2e-vm\source-windows.ps1 start
  tools\e2e-vm\source-windows.ps1 stop
  tools\e2e-vm\source-windows.ps1 ssh [command]
  tools\e2e-vm\source-windows.ps1 shot <file.png>
  tools\e2e-vm\source-windows.ps1 probe        # can QEMU use the Windows hypervisor here?

Runs on a Windows host with QEMU and the Windows Hypervisor Platform (WHPX),
which a person enables once, as administrator, followed by a reboot:

  Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All -NoRestart

The keyboard and mouse are QEMU's virtual USB devices, so to the guest they
are hardware: the low-level hooks of the KVM see them as a person typing, not
as injected input, which is the whole point of testing in a VM.

Everything lives in $env:E2E_DIR (default %LOCALAPPDATA%\oxdesk-e2e), never in
the repository: the disk, the answer file, the SSH key, the generated password
of the test account. A Windows VM needs ~25 GB at its peak, so on a machine
with a small system drive point E2E_DIR at a data disk.
#>
param(
  [Parameter(Position = 0)] [string] $Command,
  [Parameter(Position = 1, ValueFromRemainingArguments = $true)] [string[]] $Rest,
  [string] $Iso
)
$ErrorActionPreference = 'Stop'

$E2E = if ($env:E2E_DIR) { $env:E2E_DIR } else { Join-Path $env:LOCALAPPDATA 'oxdesk-e2e' }
$Name = 'source-windows'
$D = Join-Path $E2E $Name
$Key = Join-Path $E2E 'id_ed25519'
$SshPort = if ($env:SOURCE_SSH_PORT) { [int]$env:SOURCE_SSH_PORT } else { 2223 }
$QmpPort = if ($env:SOURCE_QMP_PORT) { [int]$env:SOURCE_QMP_PORT } else { 4445 }
$VncDisplay = if ($env:VNC_DISPLAY) { [int]$env:VNC_DISPLAY } else { 6 }
$MemMB = if ($env:SOURCE_MEM_MB) { [int]$env:SOURCE_MEM_MB } else { 4096 }
$Cpus = if ($env:SOURCE_CPUS) { [int]$env:SOURCE_CPUS } else { 4 }
# Overridable for a smoke test of the arguments (tcg); real runs need WHPX.
$Accel = if ($env:SOURCE_ACCEL) { $env:SOURCE_ACCEL } else { 'whpx,kernel-irqchip=off' }

function Fail([string] $Message) { Write-Host "FAIL: $Message" -ForegroundColor Red; exit 1 }

function Find-Tool([string] $Exe) {
  if ($env:QEMU_DIR) {
    $p = Join-Path $env:QEMU_DIR $Exe
    if (Test-Path $p) { return $p }
    Fail "$Exe is not in QEMU_DIR ($env:QEMU_DIR)"
  }
  $c = Get-Command $Exe -ErrorAction SilentlyContinue
  if (-not $c) { Fail "$Exe not found: install QEMU or set QEMU_DIR" }
  return $c.Source
}

# One argument string for Start-Process, which joins with spaces and quotes
# nothing on Windows PowerShell 5.1.
function Join-Args([string[]] $List) {
  ($List | ForEach-Object { if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) -join ' '
}

# A path inside a QEMU option list: commas are separators there, so a literal
# comma is doubled.
function Opt-Path([string] $Path) { ($Path -replace '\\', '/') -replace ',', ',,' }

function Qemu-Up {
  $pidFile = Join-Path $D 'pid'
  if (-not (Test-Path $pidFile)) { return $false }
  $p = Get-Process -Id ([int](Get-Content $pidFile -Raw).Trim()) -ErrorAction SilentlyContinue
  return [bool]($p -and $p.ProcessName -like 'qemu-system*')
}

# Sends QMP commands (after the capabilities handshake) and returns the
# replies, one JSON line each.
function Invoke-Qmp([int] $Port, [string[]] $Commands, [int] $TimeoutMs = 3000) {
  $client = New-Object System.Net.Sockets.TcpClient
  $client.ReceiveTimeout = $TimeoutMs
  $client.Connect('127.0.0.1', $Port)
  try {
    $stream = $client.GetStream()
    $reader = New-Object System.IO.StreamReader($stream)
    $writer = New-Object System.IO.StreamWriter($stream)
    $writer.AutoFlush = $true
    [void]$reader.ReadLine()                      # the greeting
    $writer.WriteLine('{"execute":"qmp_capabilities"}')
    [void]$reader.ReadLine()
    $out = @()
    foreach ($c in $Commands) {
      $writer.WriteLine($c)
      do { $line = $reader.ReadLine() } while ($line -and $line -match '^\{"event"')
      $out += $line
    }
    return $out
  } finally { $client.Close() }
}

# WHPX unavailable is an immediate exit with "No accelerator found", so the
# proof of availability is the opposite: a QEMU that asked for WHPX and is
# still answering on QMP. Measured both ways on the reference Windows
# machine; a probe that only checked "still alive after N s" passed while
# QEMU had in fact refused the accelerator.
function Test-Whpx {
  $qemu = Find-Tool 'qemu-system-x86_64.exe'
  $port = $QmpPort + 100
  $err = Join-Path $E2E 'whpx-probe.err'
  New-Item -ItemType Directory -Force $E2E | Out-Null
  $argList = Join-Args @('-accel', 'whpx,kernel-irqchip=off', '-machine', 'q35', '-m', '64', '-nodefaults',
    '-display', 'none', '-S', '-qmp', "tcp:127.0.0.1:$port,server=on,wait=off")
  $p = Start-Process -FilePath $qemu -ArgumentList $argList -PassThru -WindowStyle Hidden -RedirectStandardError $err
  try {
    for ($i = 0; $i -lt 16; $i++) {
      Start-Sleep -Milliseconds 500
      if ($p.HasExited) { break }
      try {
        $r = Invoke-Qmp -Port $port -Commands @('{"execute":"query-status"}')
        if ($r -and $r[0] -match '"return"') { return $true }
      } catch { }
    }
    $why = (Get-Content $err -Raw -ErrorAction SilentlyContinue)
    if ($why) { Write-Host $why.Trim() }
    return $false
  } finally {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
  }
}

function Assert-Memory {
  $freeMB = [int]((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1024)
  if ($freeMB -lt $MemMB + 1024) {
    Fail "the VM wants $MemMB MB and this machine has $freeMB MB free; close something or set SOURCE_MEM_MB"
  }
}

# The answer file goes on a small CD of its own: Windows Setup reads
# autounattend.xml only from removable media, and a CD is never a target of
# the answer file's disk wipe (a USB stick could be numbered disk 0).
# IMAPI2FS ships with Windows, so no ISO tool is needed.
function New-AnswerCd([string] $SourceDir, [string] $IsoPath) {
  if (-not ('E2eIsoWriter' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class E2eIsoWriter {
  public static void Save(object comStream, string path) {
    IStream s = (IStream)comStream;
    IntPtr read = Marshal.AllocHGlobal(4);
    try {
      using (FileStream f = File.Create(path)) {
        byte[] buf = new byte[65536];
        while (true) {
          s.Read(buf, buf.Length, read);
          int n = Marshal.ReadInt32(read);
          if (n <= 0) break;
          f.Write(buf, 0, n);
        }
      }
    } finally { Marshal.FreeHGlobal(read); }
  }
}
'@
  }
  $fsi = New-Object -ComObject IMAPI2FS.MsftFileSystemImage
  $fsi.FileSystemsToCreate = 3            # ISO 9660 + Joliet (long names)
  $fsi.VolumeName = 'OXDESK_E2E'
  $fsi.Root.AddTree($SourceDir, $false)
  [E2eIsoWriter]::Save($fsi.CreateResultImage().ImageStream, $IsoPath)
}

function New-Password {
  $chars = [char[]]'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'
  $bytes = New-Object byte[] 24
  [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
  -join ($bytes | ForEach-Object { $chars[$_ % $chars.Length] })
}

# Only the person running this can read it.
function Protect-File([string] $Path) {
  & icacls $Path /inheritance:r /grant:r "$($env:USERNAME):(R,W)" | Out-Null
}

function Write-AnswerFiles([string] $Dir, [string] $Password, [string] $PubKey) {
  New-Item -ItemType Directory -Force $Dir | Out-Null
  $xml = @'
<?xml version="1.0" encoding="utf-8"?>
<!-- Written by tools/e2e-vm/source-windows.ps1 for a throwaway test VM. -->
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">
  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <SetupUILanguage><UILanguage>en-US</UILanguage></SetupUILanguage>
      <InputLocale>en-US</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <!-- No TPM, no Secure Boot: the VM has neither, by choice (specs/026). -->
      <RunSynchronous>
        <RunSynchronousCommand wcm:action="add"><Order>1</Order><Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassTPMCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add"><Order>2</Order><Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassSecureBootCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add"><Order>3</Order><Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassRAMCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add"><Order>4</Order><Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassCPUCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
        <RunSynchronousCommand wcm:action="add"><Order>5</Order><Path>reg add HKLM\SYSTEM\Setup\LabConfig /v BypassStorageCheck /t REG_DWORD /d 1 /f</Path></RunSynchronousCommand>
      </RunSynchronous>
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>Primary</Type><Size>500</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>2</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Format>NTFS</Format><Label>System</Label><Active>true</Active></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>2</Order><PartitionID>2</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallTo><DiskID>0</DiskID><PartitionID>2</PartitionID></InstallTo>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <!-- Microsoft's published KMS client setup key for Windows 11 Pro
             (learn.microsoft.com, "KMS client activation and product keys"):
             it picks the edition and does not activate. The VM stays
             unactivated, as decided in specs/026. -->
        <ProductKey><Key>W269N-WFGWX-YVC9B-4J6C9-T83GX</Key><WillShowUI>OnError</WillShowUI></ProductKey>
      </UserData>
    </component>
  </settings>
  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>e2e-source</ComputerName>
      <TimeZone>UTC</TimeZone>
    </component>
  </settings>
  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <InputLocale>en-US</InputLocale>
      <SystemLocale>en-US</SystemLocale>
      <UILanguage>en-US</UILanguage>
      <UserLocale>en-US</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideOEMRegistrationScreen>true</HideOEMRegistrationScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <ProtectYourPC>3</ProtectYourPC>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>tester</Name>
            <Group>Administrators</Group>
            <Password><Value>__PASSWORD__</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>
      <AutoLogon>
        <Enabled>true</Enabled>
        <Username>tester</Username>
        <Password><Value>__PASSWORD__</Value><PlainText>true</PlainText></Password>
        <LogonCount>9999999</LogonCount>
      </AutoLogon>
      <FirstLogonCommands>
        <SynchronousCommand wcm:action="add">
          <Order>1</Order>
          <CommandLine>powershell -NoProfile -ExecutionPolicy Bypass -Command "foreach ($d in Get-PSDrive -PSProvider FileSystem) { $f = Join-Path $d.Root 'e2e-firstlogon.ps1'; if (Test-Path $f) { &amp; $f; break } }"</CommandLine>
          <Description>oxdesk e2e: SSH, no sleep, no lock</Description>
        </SynchronousCommand>
      </FirstLogonCommands>
    </component>
  </settings>
</unattend>
'@
  $xml = $xml.Replace('__PASSWORD__', $Password)
  [System.IO.File]::WriteAllText((Join-Path $Dir 'autounattend.xml'), $xml, (New-Object System.Text.UTF8Encoding($false)))

  $first = @'
# oxdesk e2e: runs once, at the first automatic logon of the source VM.
$ErrorActionPreference = 'Continue'
$log = 'C:\e2e-firstlogon.log'
function Say([string] $m) { Add-Content $log ("{0} {1}" -f (Get-Date -Format s), $m) }
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Say "start, answer CD at $here"

# Never sleep, blank or lock: a screen that turns off stops the capture, and
# a lock screen takes the input desktop away from the KVM.
powercfg /change monitor-timeout-ac 0
powercfg /change standby-timeout-ac 0
powercfg /change hibernate-timeout-ac 0
reg add "HKCU\Control Panel\Desktop" /v ScreenSaveActive /t REG_SZ /d 0 /f | Out-Null
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows\Personalization" /v NoLockScreen /t REG_DWORD /d 1 /f | Out-Null
# No update and no reboot in the middle of a test run.
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU" /v NoAutoUpdate /t REG_DWORD /d 1 /f | Out-Null

# SSH, key only, PowerShell as the shell: how the harness reads this VM.
if (-not (Get-Service sshd -ErrorAction SilentlyContinue)) {
  Say 'installing the OpenSSH server'
  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0 | Out-Null
}
Start-Service sshd          # creates C:\ProgramData\ssh on its first start
$keys = 'C:\ProgramData\ssh\administrators_authorized_keys'
Copy-Item (Join-Path $here 'e2e.pub') $keys -Force
icacls $keys /inheritance:r /grant '*S-1-5-32-544:F' /grant '*S-1-5-18:F' | Out-Null
New-ItemProperty HKLM:\SOFTWARE\OpenSSH -Name DefaultShell -PropertyType String -Force `
  -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' | Out-Null
Set-Service sshd -StartupType Automatic
Restart-Service sshd
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue)) {
  New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)' -Enabled True `
    -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 | Out-Null
}
Say ("sshd: {0}" -f (Get-Service sshd).Status)
Set-Content 'C:\e2e-ready.txt' (Get-Date -Format s)
Say 'done'
'@
  [System.IO.File]::WriteAllText((Join-Path $Dir 'e2e-firstlogon.ps1'), $first, (New-Object System.Text.UTF8Encoding($false)))
  [System.IO.File]::WriteAllText((Join-Path $Dir 'e2e.pub'), $PubKey.Trim() + "`n", (New-Object System.Text.UTF8Encoding($false)))
}

function Start-Vm([string] $InstallIso, [string] $AnswerIso) {
  if (Qemu-Up) { Write-Host "already running (pid $(Get-Content (Join-Path $D 'pid')))"; return }
  Assert-Memory
  $qemu = Find-Tool 'qemu-system-x86_64.exe'
  $a = @(
    '-name', $Name,
    '-machine', 'q35', '-accel', $Accel,
    '-smp', "$Cpus", '-m', "$MemMB", '-rtc', 'base=localtime',
    '-drive', "file=$(Opt-Path (Join-Path $D 'disk.qcow2')),if=none,id=sys,format=qcow2,discard=unmap",
    '-device', 'ide-hd,drive=sys,bus=ide.0,bootindex=1',
    '-device', 'VGA,edid=on,xres=1600,yres=900',
    '-display', "vnc=127.0.0.1:$VncDisplay",
    # Relative mouse, like a real one: the grab works on deltas.
    '-device', 'qemu-xhci,id=xhci', '-device', 'usb-kbd,id=kbd0,bus=xhci.0', '-device', 'usb-mouse,id=mouse0,bus=xhci.0',
    '-netdev', "user,id=net0,hostfwd=tcp:127.0.0.1:$SshPort-:22", '-device', 'e1000e,netdev=net0',
    '-qmp', "tcp:127.0.0.1:$QmpPort,server=on,wait=off",
    '-pidfile', (Join-Path $D 'pid')
  )
  if ($InstallIso) {
    # The installer boots first; after Setup's own reboot the CD's "press
    # any key" times out and the disk boots.
    $a += @('-drive', "file=$(Opt-Path $InstallIso),if=none,id=cd0,media=cdrom,readonly=on",
            '-device', 'ide-cd,drive=cd0,bus=ide.1,bootindex=0',
            '-drive', "file=$(Opt-Path $AnswerIso),if=none,id=cd1,media=cdrom,readonly=on",
            '-device', 'ide-cd,drive=cd1,bus=ide.2')
  }
  $log = Join-Path $D 'qemu.log'
  $p = Start-Process -FilePath $qemu -ArgumentList (Join-Args $a) -PassThru -WindowStyle Hidden `
    -RedirectStandardError $log -RedirectStandardOutput (Join-Path $D 'qemu.out')
  Start-Sleep -Seconds 3
  if ($p.HasExited) { Fail "qemu exited: $((Get-Content $log -Raw).Trim())" }
  Write-Host "booted (pid $($p.Id)); screen: VNC 127.0.0.1:$(5900 + $VncDisplay)"
}

function Invoke-VmSsh([string[]] $CommandLine) {
  $sshArgs = @('-i', $Key, '-p', "$SshPort", '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=NUL',
    '-o', 'LogLevel=ERROR', '-o', 'ConnectTimeout=5', '-o', 'BatchMode=yes', 'tester@127.0.0.1') + $CommandLine
  & ssh @sshArgs
}

function Wait-Ssh([int] $Minutes) {
  $until = (Get-Date).AddMinutes($Minutes)
  while ((Get-Date) -lt $until) {
    if (-not (Qemu-Up)) { Fail "qemu is gone (see $D\qemu.log)" }
    $r = Invoke-VmSsh @('Test-Path C:\e2e-ready.txt') 2>$null
    if ($r -eq 'True') { return $true }
    Start-Sleep -Seconds 20
  }
  return $false
}

# Dot-sourced (`. source-windows.ps1`), it only defines the functions.
if ($MyInvocation.InvocationName -eq '.') { return }

switch ($Command) {
  'probe' {
    if (Test-Whpx) { Write-Host 'WHPX: available' } else { Fail 'WHPX: not available (enable HypervisorPlatform and reboot)' }
  }
  'build' {
    if (-not $Iso -or -not (Test-Path $Iso)) { Fail 'build needs -Iso <path to a Windows 11 x64 ISO>' }
    if (Qemu-Up) { Fail 'it is running; stop it first' }
    if (-not (Test-Whpx)) {
      Fail 'QEMU cannot use the Windows hypervisor. As administrator: Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All -NoRestart; then reboot'
    }
    New-Item -ItemType Directory -Force $D | Out-Null
    if (-not (Test-Path $Key)) { & ssh-keygen -q -t ed25519 -N '""' -C 'oxdesk-e2e' -f $Key; if (-not (Test-Path $Key)) { Fail 'ssh-keygen' } }
    # The test account's password is generated here and kept next to the
    # disk: no credential lives in the repository, not even a throwaway one.
    $pwFile = Join-Path $D 'password'
    if (-not (Test-Path $pwFile)) { Set-Content -Path $pwFile -Value (New-Password) -NoNewline; Protect-File $pwFile }
    $answers = Join-Path $D 'answers'
    if (Test-Path $answers) { Remove-Item -Recurse -Force $answers }
    Write-AnswerFiles -Dir $answers -Password (Get-Content $pwFile -Raw) -PubKey (Get-Content "$Key.pub" -Raw)
    $answerIso = Join-Path $D 'answers.iso'
    New-AnswerCd -SourceDir $answers -IsoPath $answerIso
    Protect-File $answerIso
    Remove-Item -Recurse -Force $answers      # the password stays only in the ISO and the file above
    $disk = Join-Path $D 'disk.qcow2'
    if (Test-Path $disk) { Remove-Item -Force $disk }
    & (Find-Tool 'qemu-img.exe') create -q -f qcow2 $disk 64G
    if (-not (Test-Path $disk)) { Fail 'qemu-img' }
    Start-Vm -InstallIso (Resolve-Path $Iso).Path -AnswerIso $answerIso
    Write-Host 'installing Windows (unattended; ~30 min, then the first logon sets up SSH)...'
    if (-not (Wait-Ssh 120)) { Fail "no SSH after 2 h (screenshot: $($MyInvocation.MyCommand.Path) shot x.png)" }
    Write-Host "built: $(Invoke-VmSsh @('(Get-CimInstance Win32_OperatingSystem).Caption + \" \" + [Environment]::OSVersion.Version'))"
  }
  'start' {
    if (-not (Test-Path (Join-Path $D 'disk.qcow2'))) { Fail 'not built: run build first' }
    Start-Vm
    if (-not (Wait-Ssh 10)) { Fail 'no SSH' }
    Write-Host "up: $(Invoke-VmSsh @('hostname'))"
  }
  'stop' {
    if (Qemu-Up) {
      Invoke-VmSsh @('Stop-Computer -Force') 2>$null
      for ($i = 0; $i -lt 60 -and (Qemu-Up); $i++) { Start-Sleep -Seconds 1 }
      if (Qemu-Up) { Stop-Process -Id ([int](Get-Content (Join-Path $D 'pid') -Raw).Trim()) -Force }
    }
    Remove-Item -Force (Join-Path $D 'pid') -ErrorAction SilentlyContinue
    Write-Host 'stopped'
  }
  'ssh' { Invoke-VmSsh $Rest }
  'shot' {
    if (-not $Rest) { Fail 'usage: shot <file.png>' }
    $out = [System.IO.Path]::GetFullPath($Rest[0]) -replace '\\', '/'
    Invoke-Qmp -Port $QmpPort -Commands @("{`"execute`":`"screendump`",`"arguments`":{`"filename`":`"$out`",`"format`":`"png`"}}")
  }
  default {
    (Get-Content $MyInvocation.MyCommand.Path | Select-Object -Skip 3 -First 13) -join "`n"
    exit 2
  }
}
