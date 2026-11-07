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

[ "$(id -u)" = 0 ] || { echo "needs root: sudo bash $0"; exit 1; }
[ -n "${SUDO_USER:-}" ] || { echo "run with sudo, not as root directly"; exit 1; }

LABEL=ai.logi.tailscaled
PLIST="/Library/LaunchDaemons/$LABEL.plist"
STATE=/var/db/tailscale-tun
SOCK=/var/run/tailscaled.socket

PREFIX=$(sudo -u "$SUDO_USER" -i brew --prefix 2>/dev/null)
TS="$PREFIX/bin/tailscale"
TSD="$PREFIX/bin/tailscaled"
[ -x "$TSD" ] && [ -x "$TS" ] || { echo "tailscale from brew is missing: brew install tailscale"; exit 1; }

if [ -e /Applications/Tailscale.app ] || scutil --nc list | grep -qi tailscale; then
  echo "the Tailscale app is still here. Delete it in Finder (macOS removes the extension"
  echo "with it), remove the VPN in System Settings > VPN, and run again."
  exit 1
fi

IFACE=$(route -n get default 2>/dev/null | awk '/interface:/{print $2}')
[ -n "$IFACE" ] || { echo "this machine has no default route: nothing was changed"; exit 1; }

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
  echo ">> rolling back: $1"
  launchctl bootout "system/$LABEL" 2>/dev/null
  rm -f "$PLIST"
  sleep 3
  net_ok && echo ">> internet ok, nothing left installed" || echo ">> WARNING: the internet still fails"
  exit 1
}

net_ok || { echo "the internet already fails before starting: nothing was changed"; exit 1; }

if [ "$(backend)" = Running ]; then
  echo "Tailscale is already up on this machine: nothing to do"
  exit 0
fi

echo "==> service"
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
launchctl bootstrap system "$PLIST" || rollback "launchd refused the service"

for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 2
  [ -n "$(backend)" ] && break
  [ "$i" = 10 ] && rollback "the service did not respond within 20 s"
done

echo "==> login: open the link that appears"
"$TS" --socket="$SOCK" up --accept-routes=false --accept-dns=false --operator="$SUDO_USER" ||
  rollback "the login did not finish"
[ "$(backend)" = Running ] || rollback "the service never reached Running"

echo "==> checking the internet with the tunnel up"
for i in 1 2 3 4; do sleep 5; net_ok || rollback "the internet failed with the tunnel up"; done

echo "==> tailnet names"
"$TS" --socket="$SOCK" set --accept-dns=true
for i in 1 2 3; do
  sleep 4
  net_ok || {
    "$TS" --socket="$SOCK" set --accept-dns=false
    echo ">> the names broke the internet: turned them off. Use the 100.x addresses."
    break
  }
done

echo "tunnel: $(ifconfig | awk '/^[a-z]/{n=$1} /inet 100\./{print n, $2}')"
echo "default route: $(route -n get default | awk '/interface:/{print $2}')"
echo "DONE"
