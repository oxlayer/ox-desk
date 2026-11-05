# O que esta no clipboard da SESSAO INTERATIVA.
#
# Pelo SSH se le a sessao 0, que tem clipboard proprio e sempre vazio --
# numero invalido toda vez. Daí a tarefa com /IT.
$out = "$env:LOCALAPPDATA\logid\clipboard.txt"
$linhas = @()
$arq = Get-Clipboard -Format FileDropList -ErrorAction SilentlyContinue
if ($arq) { foreach ($a in $arq) { $linhas += "ARQUIVO: $a" } }
$txt = Get-Clipboard -Format Text -ErrorAction SilentlyContinue
if ($txt) { $linhas += "TEXTO: $txt" }
if ($linhas.Count -eq 0) { $linhas += "(vazio)" }
$linhas -join "`r`n" | Set-Content -Path $out -Encoding utf8
