# What is on the clipboard of the INTERACTIVE SESSION.
#
# Over SSH you read session 0, which has its own clipboard and it is always
# empty -- an invalid reading every time. Hence the task with /IT.
$out = "$env:LOCALAPPDATA\logid\clipboard.txt"
$linhas = @()
$arq = Get-Clipboard -Format FileDropList -ErrorAction SilentlyContinue
if ($arq) { foreach ($a in $arq) { $linhas += "ARQUIVO: $a" } }
$txt = Get-Clipboard -Format Text -ErrorAction SilentlyContinue
if ($txt) { $linhas += "TEXTO: $txt" }
if ($linhas.Count -eq 0) { $linhas += "(vazio)" }
$linhas -join "`r`n" | Set-Content -Path $out -Encoding utf8
