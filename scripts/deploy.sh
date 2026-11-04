#!/usr/bin/env bash
# Compila, assina e distribui o daemon para as tres maquinas, e confere
# que as tres ficaram no MESMO commit.
#
# Existe porque sincronizar na mao falhou: o Mac ficou um dia inteiro com
# binario velho e quem notou foi o usuario, nao eu ("nao vejo as coisas
# novas no do mac"). Tres alvos, dois deles remotos, um precisando de
# assinatura -- isso nao e trabalho de memoria.
#
#   uso:  bash scripts/deploy.sh [--pular-mac]
set -uo pipefail
cd "$(dirname "$0")/.."
export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"

PULAR_MAC=0
[ "${1:-}" = "--pular-mac" ] && PULAR_MAC=1

BUILD="$(git rev-parse --short HEAD 2>/dev/null || echo desconhecido)"
git diff --quiet 2>/dev/null || BUILD="$BUILD-sujo"
export LOGI_BUILD="$BUILD"
# Numero incremental: diz QUAL maquina esta atras, nao so que estao
# diferentes. Viaja junto porque a copia do Mac nao tem .git.
LOGI_BUILD_N="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
export LOGI_BUILD_N
echo "== build #$LOGI_BUILD_N $BUILD =="

falhas=0

# Um passo "ok" tem de significar que nada deu errado -- nem o nosso
# `ERRO` explicito, nem o shell reclamando.
#
# Quebrei o passo do host de tela com uma crase dentro de heredoc
# expandido (o bash executou `r` como comando) e o passo disse **ok**: o
# erro saiu em stderr, fora da variavel que o teste lia. Passo que nao
# captura stderr nao consegue falhar, e passo que nao consegue falhar nao
# e verificacao -- e decoracao.
ruim() {
  printf '%s' "$1" | grep -qE 'ERRO|command not found|No such file|unbound variable|cannot'
}

passo() { printf '  %-44s' "$1"; }
ok()    { echo "ok"; }
erro()  { echo "FALHOU"; echo "$1" | tail -6 | sed 's/^/      /'; falhas=$((falhas+1)); }

# Roda um script PowerShell no Windows sem passar por quoting de shell.
# -EncodedCommand nao tem quoting nenhum; aspas e parenteses dentro de
# -Command quebram o parse, e o PowerShell valida o script TODO antes de
# executar, entao uma linha ruim no fim faz nenhuma rodar.
psrun() {
  local enc; enc=$(iconv -f UTF-8 -t UTF-16LE | base64 -w0)
  timeout 180 ssh -o BatchMode=yes windows-host "powershell -NoProfile -EncodedCommand $enc" 2>&1 |
    grep -av 'post-quantum\|store now\|openssh.com/pq\|^\*\* \|#< CLIXML\|<Objs Version' | tr -d '\r'
}

# ---------- ubuntu ----------
passo "ubuntu: compilar"
if out=$(cargo build --release --manifest-path logid/Cargo.toml 2>&1); then ok; else erro "$out"; fi
passo "ubuntu: instalar e reiniciar"
if out=$(install -m755 logid/target/release/logid ~/.local/bin/logid 2>&1 &&
         systemctl --user restart logid.service 2>&1); then ok; else erro "$out"; fi

passo "ubuntu: tela e KVM (logistream)"
# O host roda nesta maquina quando alguem pede a tela dela; o cliente
# tambem fica instalado para o caso de ela virar quem assiste.
#
# O `logistream-kvm` tem de estar aqui porque o `logid` o lanca por
# nome quando a bandeja manda o input para outra maquina. Ele ficou de
# fora na primeira versao e o botao falhava com "nao consegui executar
# logistream-kvm" -- binario compilado, nunca instalado.
if out=$(cargo build --release --manifest-path logistream/Cargo.toml 2>&1 &&
         install -m755 logistream/target/release/logistream-host ~/.local/bin/ 2>&1 &&
         install -m755 logistream/target/release/logistream-view ~/.local/bin/ 2>&1 &&
         install -m755 logistream/target/release/logistream-kvm ~/.local/bin/ 2>&1 &&
         mkdir -p ~/.local/state 2>&1); then ok; else erro "$out"; fi

# ---------- windows ----------
passo "windows: compilar"
if out=$(cargo build --release --target x86_64-pc-windows-gnu --manifest-path logid/Cargo.toml 2>&1); then ok; else erro "$out"; fi

EXE=logid/target/x86_64-pc-windows-gnu/release/logid.exe
ASSINADO=$(mktemp -u /tmp/logid-assinado-XXXX.exe)
passo "windows: assinar"
# SEMPRE antes de copiar: o Defender removeu o .exe cru no instante da
# escrita, duas vezes, levando a tarefa agendada junto. Ver specs/008.
if out=$(cp "$EXE" "$ASSINADO" && bash scripts/sign-windows.sh "$ASSINADO" 2>&1); then ok; else erro "$out"; fi

passo "windows: enviar e trocar"
if out=$(timeout 120 scp -q "$ASSINADO" 'windows-host:C:/Users/user/AppData/Local/logid/logid.new.exe' 2>&1); then
  out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logid.new.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ("ERRO: assinatura " + $v.Status); exit 1 }
Get-Process -Name logid -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600
# Tamanho ANTES de mover, comparado DEPOIS. Em PowerShell, nao
# passado pelo bash: assinatura valida nao garante que o arquivo ficou
# (o Defender reverteu a gravacao do agente em silencio por uma hora
# enquanto este script dizia "ok"), e trazer o numero de fora exigiria
# heredoc expandido -- que foi como eu quebrei este mesmo passo.
$esperado = (Get-Item $novo).Length
Copy-Item -Force $novo (Join-Path $dst 'logid.exe')
Remove-Item -Force $novo
Start-Sleep -Milliseconds 500
$tem = (Get-Item (Join-Path $dst 'logid.exe')).Length
if ($tem -ne $esperado) {
  Write-Output ('ERRO: gravou mas tem ' + $tem + ' bytes, esperado ' + $esperado + ' -- antivirus revertendo? confira a exclusao de ' + $dst)
  exit 1
}
schtasks /run /tn 'logid' | Out-Null
exit 0
PS1
)
  if ruim "$out"; then erro "$out"; else ok; fi
else erro "$out"; fi
rm -f "$ASSINADO"

passo "windows: casca da GUI"
# So o `main.js`: o resto da casca e `node_modules` (299 MB de Chromium)
# e nao muda. O arquivo passou a importar porque e nele que mora o vigia
# de build -- a pagina tambem sabe se recarregar, mas esse codigo VIAJA
# NA PAGINA, e uma janela aberta antes de o recurso existir nunca o
# recebe. Foi o que deixou o usuario olhando um painel de ontem.
#
# Vale no proximo lancamento do app; nao matamos a janela de ninguem.
if out=$(timeout 60 scp -q gui/main.js \
           'windows-host:C:/Users/user/AppData/Local/logid/gui/main.js' 2>&1); then ok
else erro "$out"; fi

passo "windows: agente"
# O agente FICOU PARA TRAS por um dia inteiro: este script atualizava
# `logid.exe` e esquecia `logid-agent.exe`, que e justamente quem o
# Ubuntu invoca por SSH para ler dispositivos e pedir troca de canal.
# Binario velho do outro lado da ponte e o tipo de divergencia que nao
# da erro -- responde, so responde o de ontem.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path agent/Cargo.toml 2>&1); then
  AG=agent/target/x86_64-pc-windows-gnu/release/logid-agent.exe
  AGSIG=$(mktemp -u /tmp/logid-agent-assinado-XXXX.exe)
  if out=$(cp "$AG" "$AGSIG" && bash scripts/sign-windows.sh "$AGSIG" 2>&1 &&
           timeout 120 scp -q "$AGSIG" \
             'windows-host:C:/Users/user/AppData/Local/logid/logid-agent.new.exe' 2>&1); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logid-agent.new.exe'
$alvo = Join-Path $dst 'logid-agent.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ('ERRO: assinatura ' + $v.Status); exit 1 }
# O agente e invocado e morre; nao ha processo residente para parar.
# Tamanho ANTES de mover, comparado DEPOIS. Em PowerShell, nao
# passado pelo bash: assinatura valida nao garante que o arquivo ficou
# (o Defender reverteu a gravacao do agente em silencio por uma hora
# enquanto este script dizia "ok"), e trazer o numero de fora exigiria
# heredoc expandido -- que foi como eu quebrei este mesmo passo.
$esperado = (Get-Item $novo).Length
Copy-Item -Force $novo $alvo
Remove-Item -Force $novo
Start-Sleep -Milliseconds 500
$tem = (Get-Item $alvo).Length
if ($tem -ne $esperado) {
  Write-Output ('ERRO: gravou mas tem ' + $tem + ' bytes, esperado ' + $esperado + ' -- antivirus revertendo? confira a exclusao de ' + $dst)
  exit 1
}
exit 0
PS1
)
    if ruim "$out"; then erro "$out"; else ok; fi
  else erro "$out"; fi
  rm -f "$AGSIG"
else erro "$out"; fi

passo "windows: visualizador de tela"
# O visualizador deixou de ser so do Mac quando o NVDEC passou a
# decodificar -- e e ele que tira o ultimo RDP do sistema. Sem
# `libavcodec` do lado de la: a lista de DLLs do .exe traz so coisa do
# Windows, e o decodificador vem com o driver NVIDIA, carregado em
# runtime.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path logistream/Cargo.toml --bin logistream-view 2>&1); then
  VW=logistream/target/x86_64-pc-windows-gnu/release/logistream-view.exe
  VWSIG=$(mktemp -u /tmp/logistream-view-assinado-XXXX.exe)
  if out=$(cp "$VW" "$VWSIG" && bash scripts/sign-windows.sh "$VWSIG" 2>&1 &&
           timeout 180 scp -q "$VWSIG" \
             'windows-host:C:/Users/user/AppData/Local/logid/logistream-view.new.exe' 2>&1); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logistream-view.new.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ('ERRO: assinatura ' + $v.Status); exit 1 }
# Pode estar aberto: quem assiste fecha a janela quando quiser.
Get-Process -Name logistream-view -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
# Tamanho ANTES de mover, comparado DEPOIS. Em PowerShell, nao
# passado pelo bash: assinatura valida nao garante que o arquivo ficou
# (o Defender reverteu a gravacao do agente em silencio por uma hora
# enquanto este script dizia "ok"), e trazer o numero de fora exigiria
# heredoc expandido -- que foi como eu quebrei este mesmo passo.
$esperado = (Get-Item $novo).Length
Copy-Item -Force $novo (Join-Path $dst 'logistream-view.exe')
Remove-Item -Force $novo
Start-Sleep -Milliseconds 500
$tem = (Get-Item (Join-Path $dst 'logistream-view.exe')).Length
if ($tem -ne $esperado) {
  Write-Output ('ERRO: gravou mas tem ' + $tem + ' bytes, esperado ' + $esperado + ' -- antivirus revertendo? confira a exclusao de ' + $dst)
  exit 1
}
exit 0
PS1
)
    if ruim "$out"; then erro "$out"; else ok; fi
  else erro "$out"; fi
  rm -f "$VWSIG"
else erro "$out"; fi

passo "windows: host de tela (logistream)"
# O Windows virou HOST do nosso protocolo (v4): DXGI -> NVENC -> QUIC.
# Precisa de assinatura pelo mesmo motivo que o logid -- o Defender
# apagou o .exe cru no instante da escrita, duas vezes (specs/008).
#
# E precisa de TAREFA AGENDADA, nao de lancamento por SSH: o SSH cai na
# sessao 0, e la o Desktop Duplication nao existe e o SendInput nao
# chega a desktop nenhum. `/it` faz a tarefa rodar na sessao interativa.
if out=$(cargo build --release --target x86_64-pc-windows-gnu \
           --manifest-path logistream/Cargo.toml --bin logistream-host 2>&1); then
  HOSTEXE=logistream/target/x86_64-pc-windows-gnu/release/logistream-host.exe
  HOSTSIG=$(mktemp -u /tmp/logistream-host-assinado-XXXX.exe)
  if out=$(cp "$HOSTEXE" "$HOSTSIG" && bash scripts/sign-windows.sh "$HOSTSIG" 2>&1 &&
           timeout 180 scp -q "$HOSTSIG" \
             'windows-host:C:/Users/user/AppData/Local/logid/logistream-host.new.exe' 2>&1); then
    out=$({ cat <<'PS1' | psrun; } 2>&1
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logistream-host.new.exe'
$alvo = Join-Path $dst 'logistream-host.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ('ERRO: assinatura ' + $v.Status); exit 1 }
Get-Process -Name logistream-host -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
# Tamanho ANTES de mover, comparado DEPOIS. Em PowerShell, nao
# passado pelo bash: assinatura valida nao garante que o arquivo ficou
# (o Defender reverteu a gravacao do agente em silencio por uma hora
# enquanto este script dizia "ok"), e trazer o numero de fora exigiria
# heredoc expandido -- que foi como eu quebrei este mesmo passo.
$esperado = (Get-Item $novo).Length
Copy-Item -Force $novo $alvo
Remove-Item -Force $novo
Start-Sleep -Milliseconds 500
$tem = (Get-Item $alvo).Length
if ($tem -ne $esperado) {
  Write-Output ('ERRO: gravou mas tem ' + $tem + ' bytes, esperado ' + $esperado + ' -- antivirus revertendo? confira a exclusao de ' + $dst)
  exit 1
}
# A tarefa roda o .exe DIRETO, sem envelope .cmd.
#
# Havia um .cmd aqui so para redirecionar a saida, porque schtasks nao
# e shell. O preco era uma janela de console visivel na sessao
# interativa -- bem no meio da tela que alguem esta assistindo. O
# usuario reportou como "esse terminal que abre ao conectar a tela".
#
# Agora o host e `windows_subsystem = "windows"` (como o logid ja era) e
# redireciona a propria saida para o mesmo arquivo de log antes da
# primeira linha. Sem cmd.exe no caminho, sem janela, mesmo log.
# Sem argumento: o host assume 9777 por padrao. Passar o numero aqui
# exigiria aninhar aspas no /tr, que ja quebrou tres vezes nesta ponte.
$env1 = $alvo
Remove-Item (Join-Path $dst 'logistream-host.cmd') -ErrorAction SilentlyContinue
# Tarefa que NAO roda sozinha: `once` as 00:00 ja passou, e so a
# acordamos com `schtasks /run`. O `/it` e o ponto -- sem ele a captura
# cai na sessao 0, onde nao ha desktop para duplicar nem desktop para o
# SendInput alcancar.
$r = schtasks /create /tn logistream-host /tr $env1 /sc once /st 00:00 /it /f 2>&1
# O codigo de saida e 0 com AVISO sobre /ST estar no passado: isso e
# esperado e desejado. Erro de verdade vem com codigo != 0.
if ($LASTEXITCODE -ne 0) { Write-Output ('ERRO: schtasks ' + $r); exit 1 }
exit 0
PS1
)
    if ruim "$out"; then erro "$out"; else ok; fi
  else erro "$out"; fi
  rm -f "$HOSTSIG"
else erro "$out"; fi

# ---------- mac ----------
if [ "$PULAR_MAC" -eq 1 ]; then
  echo "  mac: pulado"
else
  passo "mac: enviar fonte"
  # `logistream` vai junto: o Mac e o CLIENTE da tela remota, e deixar o
  # binario dele para tras ja aconteceu uma vez -- quem notou foi o
  # usuario, nao eu.
  if out=$(for d in hidpp logid logistream; do
             rsync -az --delete --exclude target -e "ssh -o BatchMode=yes" "$d" mac:~/logi/ || exit 1
           done 2>&1); then ok; else erro "$out"; fi
  passo "mac: compilar e reiniciar"
  # Sem cross-compile de Linux para macOS: exige o SDK da Apple, entao o
  # build acontece la. O LOGI_BUILD viaja junto porque a copia nao tem .git.
  if out=$(timeout 420 ssh -n -o BatchMode=yes mac \
            "cd ~/logi/logid && LOGI_BUILD='$BUILD' LOGI_BUILD_N='$LOGI_BUILD_N' ~/.cargo/bin/cargo build --release 2>&1 | tail -3 &&
             install -m755 target/release/logid ~/.local/bin/logid &&
             launchctl kickstart -k gui/\$(id -u)/ai.logi.logid" 2>&1); then ok; else erro "$out"; fi

  passo "mac: cliente de tela (logistream)"
  # PKG_CONFIG_PATH do brew: o `ffmpeg-next` so acha o libavcodec por
  # ele, e o SSH nao carrega o perfil do shell.
  if out=$(timeout 600 ssh -n -o BatchMode=yes mac \
            "cd ~/logi/logistream && export PATH=/opt/homebrew/bin:\$PATH PKG_CONFIG_PATH=/opt/homebrew/lib/pkgconfig &&
             ~/.cargo/bin/cargo build --release --bin logistream-view 2>&1 | tail -3 &&
             mkdir -p ~/.local/bin ~/.local/state &&
             install -m755 target/release/logistream-view ~/.local/bin/" 2>&1); then ok; else erro "$out"; fi
fi

# ---------- conferencia ----------
echo "== conferencia =="
sleep 6
declare -A visto
visto[ubuntu]=$(curl -s --max-time 8 http://127.0.0.1:8787/healthz 2>/dev/null)
visto[windows]=$(echo "(Invoke-WebRequest -UseBasicParsing -TimeoutSec 8 'http://127.0.0.1:8787/healthz').Content" | psrun)
[ "$PULAR_MAC" -eq 1 ] || visto[mac]=$(timeout 25 ssh -n -o BatchMode=yes mac \
  'curl -s --max-time 8 http://127.0.0.1:8787/healthz' 2>/dev/null)

for h in "${!visto[@]}"; do
  b=$(printf '%s' "${visto[$h]}" | sed -n 's/.*"build":"\([^"]*\)".*/\1/p')
  n=$(printf '%s' "${visto[$h]}" | sed -n 's/.*"n":\([0-9]*\).*/\1/p')
  if [ "$b" = "$BUILD" ]; then
    printf '  %-10s #%s %s\n' "$h" "${n:-?}" "$b"
  else
    printf '  %-10s %s  <-- FORA DE SINCRONIA (esperado %s)\n' "$h" "${b:-sem resposta}" "$BUILD"
    falhas=$((falhas+1))
  fi
done

echo
[ "$falhas" -eq 0 ] && echo "tres maquinas no mesmo build: $BUILD" || echo "$falhas problema(s)"
exit "$falhas"
