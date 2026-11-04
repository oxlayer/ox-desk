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

passo "ubuntu: host de tela (logistream)"
# O host roda nesta maquina quando alguem pede a tela dela; o cliente
# tambem fica instalado para o caso de ela virar quem assiste.
if out=$(cargo build --release --manifest-path logistream/Cargo.toml 2>&1 &&
         install -m755 logistream/target/release/logistream-host ~/.local/bin/ 2>&1 &&
         install -m755 logistream/target/release/logistream-view ~/.local/bin/ 2>&1 &&
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
  out=$(cat <<'PS1' | psrun
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logid.new.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ("ERRO: assinatura " + $v.Status); exit 1 }
Get-Process -Name logid -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600
Move-Item -Force $novo (Join-Path $dst 'logid.exe')
schtasks /run /tn 'logid' | Out-Null
exit 0
PS1
)
  if echo "$out" | grep -q ERRO; then erro "$out"; else ok; fi
else erro "$out"; fi
rm -f "$ASSINADO"

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
    out=$(cat <<'PS1' | psrun
$dst = Join-Path $env:LOCALAPPDATA 'logid'
$novo = Join-Path $dst 'logistream-host.new.exe'
$alvo = Join-Path $dst 'logistream-host.exe'
if (-not (Test-Path $novo)) { Write-Output 'ERRO: o arquivo nao chegou (antivirus?)'; exit 1 }
$v = Get-AuthenticodeSignature $novo
if ($v.Status -ne 'Valid') { Write-Output ('ERRO: assinatura ' + $v.Status); exit 1 }
Get-Process -Name logistream-host -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
Move-Item -Force $novo $alvo
# Envelope .cmd: a tarefa agendada nao e um shell, entao redirecionar a
# saida exige um. Um arquivo resolve sem aninhar aspas -- e aninhar
# aspas nesta ponte ja quebrou tres vezes (specs/002).
$log = Join-Path $dst 'logistream-host.log'
$env1 = Join-Path $dst 'logistream-host.cmd'
$corpo = @"
@echo off
"$alvo" 9777 > "$log" 2>&1
"@
Set-Content -Path $env1 -Value $corpo -Encoding ASCII
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
    if echo "$out" | grep -q ERRO; then erro "$out"; else ok; fi
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
