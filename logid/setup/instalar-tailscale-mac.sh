#!/bin/bash
# Puts this Mac in the overlay without the Tailscale app.
#
# Spec: ../specs/012-outside-the-lan.md
#
#   brew install tailscale
#   sudo bash logid/setup/instalar-tailscale-mac.sh
#
# WHY NOT THE APP
#
# The app's network extension, with no login finished, sat in "Connecting"
# with Connect on demand on and the whole Mac lost its internet; the login
# page itself could not load. tailscaled from brew, as root, opens a real
# utun and only adds the 100.64.0.0/10 route.
#
# If the internet stops working at any step, this script undoes that step.
set -u

[ "$(id -u)" = 0 ] || { echo "precisa de root: sudo bash $0"; exit 1; }
[ -n "${SUDO_USER:-}" ] || { echo "rode com sudo, não como root direto"; exit 1; }

LABEL=ai.logi.tailscaled
PLIST="/Library/LaunchDaemons/$LABEL.plist"
STATE=/var/db/tailscale-tun
SOCK=/var/run/tailscaled.socket

PREFIX=$(sudo -u "$SUDO_USER" -i brew --prefix 2>/dev/null)
TS="$PREFIX/bin/tailscale"
TSD="$PREFIX/bin/tailscaled"
[ -x "$TSD" ] && [ -x "$TS" ] || { echo "falta o brew: brew install tailscale"; exit 1; }

if [ -e /Applications/Tailscale.app ] || scutil --nc list | grep -qi tailscale; then
  echo "o app Tailscale ainda está aqui. Apague-o pelo Finder (o macOS remove a extensão"
  echo "junto), remova a VPN em Ajustes > VPN, e rode de novo."
  exit 1
fi

IFACE=$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')
[ -n "$IFACE" ] || { echo "esta máquina não tem rota padrão: nada foi alterado"; exit 1; }

net_ok() {
  local n=0 h
  for h in https://github.com https://www.cloudflare.com https://www.apple.com; do
    [ "$(curl -s -o /dev/null -m 6 -w '%{http_code}' "$h")" != "000" ] && n=$((n+1))
  done
  [ "$n" -ge 2 ] && [ "$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')" = "$IFACE" ]
}

backend() {
  "$TS" --socket="$SOCK" status --json 2>/dev/null |
    python3 -c 'import sys,json;print(json.load(sys.stdin).get("BackendState",""))' 2>/dev/null
}

rollback() {
  echo ">> desfazendo: $1"
  launchctl bootout "system/$LABEL" 2>/dev/null
  rm -f "$PLIST"
  sleep 3
  net_ok && echo ">> internet ok, nada ficou instalado" || echo ">> ATENÇÃO: a internet ainda falha"
  exit 1
}

net_ok || { echo "a internet já falha antes de começar: nada foi alterado"; exit 1; }

if [ "$(backend)" = Running ]; then
  echo "o Tailscale já está de pé nesta máquina: nada a fazer"
  exit 0
fi

echo "==> serviço"
install -d -m 700 -o root -g wheel "$STATE"
cat > "$PLIST" <<PLISTEND
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array>
<string>$TSD</string><string>--tun=utun</string>
<string>--statedir=$STATE</string><string>--socket=$SOCK</string><string>--port=41641</string>
</array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>StandardOutPath</key><string>/var/log/tailscaled-tun.log</string>
<key>StandardErrorPath</key><string>/var/log/tailscaled-tun.log</string>
</dict></plist>
PLISTEND
chown root:wheel "$PLIST"; chmod 644 "$PLIST"
launchctl bootstrap system "$PLIST" || rollback "o launchd recusou o serviço"

for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 2
  [ -n "$(backend)" ] && break
  [ "$i" = 10 ] && rollback "o serviço não respondeu em 20 s"
done

echo "==> login: abra o link que aparecer"
"$TS" --socket="$SOCK" up --accept-routes=false --accept-dns=false --operator="$SUDO_USER" ||
  rollback "o login não terminou"
[ "$(backend)" = Running ] || rollback "o serviço não chegou a Running"

echo "==> conferindo a internet com o túnel ligado"
for i in 1 2 3 4; do sleep 5; net_ok || rollback "a internet falhou com o túnel ligado"; done

echo "==> nomes do tailnet"
"$TS" --socket="$SOCK" set --accept-dns=true
for i in 1 2 3; do
  sleep 4
  net_ok || {
    "$TS" --socket="$SOCK" set --accept-dns=false
    echo ">> os nomes derrubaram a internet: desliguei. Use os endereços 100.x."
    break
  }
done

echo "túnel: $(ifconfig | awk '/^[a-z]/{n=$1} /inet 100\./{print n, $2}')"
echo "rota padrão: $(route -n get default | awk '/interface:/{print $2}')"
echo "CONCLUÍDO"
