# Copia um arquivo para o clipboard da SESSAO INTERATIVA.
# Existe para testar o sentido Windows -> pares sem precisar de alguem
# clicando no Explorer. Pelo SSH nao serve: la e a sessao 0.
$alvo = "$env:LOCALAPPDATA\logid\origem-windows.txt"
"arquivo nascido no windows" | Set-Content -Path $alvo -Encoding utf8
Set-Clipboard -Path $alvo
