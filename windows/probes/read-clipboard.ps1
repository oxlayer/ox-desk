# What is on the clipboard of the INTERACTIVE SESSION.
#
# Over SSH you read session 0, which has its own clipboard and it is always
# empty -- an invalid reading every time. Hence the task with /IT.
$out = "$env:LOCALAPPDATA\oxdesk\clipboard.txt"
$lines = @()
$files = Get-Clipboard -Format FileDropList -ErrorAction SilentlyContinue
if ($files) { foreach ($a in $files) { $lines += "FILE: $a" } }
$txt = Get-Clipboard -Format Text -ErrorAction SilentlyContinue
if ($txt) { $lines += "TEXT: $txt" }
if ($lines.Count -eq 0) { $lines += "(empty)" }
$lines -join "`r`n" | Set-Content -Path $out -Encoding utf8
