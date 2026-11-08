# Gives Windows a virtual display, so the screen host has something to capture
# with no monitor plugged in. Run as administrator; works over SSH.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File oxdesk\setup\install-virtual-display.ps1
#
# Virtual Display Driver (MIT), signed by SignPath Foundation. The version and
# hash are pinned: bumping them is a reviewed change, not a download of "latest".
# Running it again changes nothing.
$ErrorActionPreference = 'Stop'
$Url  = 'https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip'
$Sha  = 'e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a'
$HwId = 'Root\MttVDD'
$Signer = 'SignPath Foundation'
$Settings = 'C:\VirtualDisplayDriver'

function Get-VirtualDisplay {
  Get-PnpDevice -Class Display -ErrorAction SilentlyContinue |
    Where-Object { $_.HardwareID -contains $HwId -and $_.Present }
}

$d = Get-VirtualDisplay
if ($d -and $d.Status -eq 'OK') { "virtual display already installed: $($d.FriendlyName)"; exit 0 }

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).
  IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) { throw 'needs administrator' }

$tmp = Join-Path $env:TEMP ('vdd-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
$trusted = $null
try {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  $zip = Join-Path $tmp 'vdd.zip'
  Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $zip
  $got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
  if ($got -ne $Sha) { throw "package hash does not match: $got" }
  Expand-Archive -Path $zip -DestinationPath $tmp
  $dir = Join-Path $tmp 'VirtualDisplayDriver'
  $inf = Join-Path $dir 'MttVDD.inf'

  $sig = Get-AuthenticodeSignature (Join-Path $dir 'mttvdd.cat')
  if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notlike "*$Signer*") {
    throw "catalog signature rejected: $($sig.Status) $($sig.SignerCertificate.Subject)"
  }
  "signature ok: $($sig.SignerCertificate.Subject)"

  # Without the publisher in this store, Windows asks on a dialog that nobody
  # sees over SSH. It stays only for the install: see `finally`.
  $store = New-Object Security.Cryptography.X509Certificates.X509Store('TrustedPublisher', 'LocalMachine')
  $store.Open('ReadWrite')
  if (-not ($store.Certificates | Where-Object Thumbprint -eq $sig.SignerCertificate.Thumbprint)) {
    $store.Add($sig.SignerCertificate); $trusted = $sig.SignerCertificate
  }
  $store.Close()

  New-Item -ItemType Directory -Force -Path $Settings | Out-Null
  # The driver starts at the first resolution listed, and 800x600 is no use
  # for a remote desktop. Windows keeps whatever mode it took first.
  $xml = [xml](Get-Content -Raw (Join-Path $dir 'vdd_settings.xml'))
  foreach ($r in @($xml.SelectNodes('//resolutions/resolution'))) {
    if ([int]$r.width -lt 1920) { [void]$r.ParentNode.RemoveChild($r) } else { $r.refresh_rate = '60' }
  }
  $xml.Save((Join-Path $Settings 'vdd_settings.xml'))

  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Vdd {
  [StructLayout(LayoutKind.Sequential)]
  struct Info { public int cb; public Guid cls; public uint inst; public IntPtr rsv; }
  [DllImport("setupapi.dll", SetLastError=true)]
  static extern IntPtr SetupDiCreateDeviceInfoList(ref Guid g, IntPtr h);
  [DllImport("setupapi.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  static extern bool SetupDiCreateDeviceInfoW(IntPtr s, string name, ref Guid g, string d, IntPtr h, uint f, ref Info i);
  [DllImport("setupapi.dll", SetLastError=true)]
  static extern bool SetupDiSetDeviceRegistryPropertyW(IntPtr s, ref Info i, uint p, byte[] b, uint n);
  [DllImport("setupapi.dll", SetLastError=true)]
  static extern bool SetupDiCallClassInstaller(uint f, IntPtr s, ref Info i);
  [DllImport("setupapi.dll")]
  static extern bool SetupDiDestroyDeviceInfoList(IntPtr s);
  [DllImport("newdev.dll", SetLastError=true, CharSet=CharSet.Unicode)]
  static extern bool UpdateDriverForPlugAndPlayDevicesW(IntPtr h, string id, string inf, uint f, out bool reboot);
  static void Must(bool ok, string step) {
    if (!ok) throw new Exception(step + " failed: " + Marshal.GetLastWin32Error());
  }
  public static void CreateNode(string hwid) {
    Guid g = new Guid("4d36e968-e325-11ce-bfc1-08002be10318");
    IntPtr s = SetupDiCreateDeviceInfoList(ref g, IntPtr.Zero);
    try {
      Info i = new Info(); i.cb = Marshal.SizeOf(typeof(Info));
      Must(SetupDiCreateDeviceInfoW(s, "Display", ref g, null, IntPtr.Zero, 1, ref i), "create node");
      byte[] id = Encoding.Unicode.GetBytes(hwid + "\0\0");
      Must(SetupDiSetDeviceRegistryPropertyW(s, ref i, 1, id, (uint)id.Length), "hardware id");
      Must(SetupDiCallClassInstaller(0x19, s, ref i), "register");
    } finally { SetupDiDestroyDeviceInfoList(s); }
  }
  public static bool Install(string hwid, string inf) {
    bool reboot;
    Must(UpdateDriverForPlugAndPlayDevicesW(IntPtr.Zero, hwid, inf, 1, out reboot), "install driver");
    return reboot;
  }
}
'@
  if (-not (Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | Where-Object { $_.HardwareID -contains $HwId })) {
    [Vdd]::CreateNode($HwId)
  }
  $reboot = [Vdd]::Install($HwId, $inf)
  if ($reboot) { 'Windows asked for a reboot to finish' }
} finally {
  if ($trusted) {
    $store = New-Object Security.Cryptography.X509Certificates.X509Store('TrustedPublisher', 'LocalMachine')
    $store.Open('ReadWrite'); $store.Remove($trusted); $store.Close()
  }
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

Start-Sleep 4
$d = Get-VirtualDisplay
if (-not $d -or $d.Status -ne 'OK') { throw "virtual display did not become OK: $($d.Status)" }
"virtual display installed: $($d.FriendlyName) [$($d.Status)]"
