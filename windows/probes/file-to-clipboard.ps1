# Copies a file to the clipboard of the INTERACTIVE SESSION.
# It exists to test the Windows -> peers direction without someone clicking
# in Explorer. Over SSH it does not work: that is session 0.
$alvo = "$env:LOCALAPPDATA\oxdesk\origem-windows.txt"
"arquivo nascido no windows" | Set-Content -Path $alvo -Encoding utf8
Set-Clipboard -Path $alvo
