#!/usr/bin/env bash
# Sends the OTA token from this machine to the other two, and proves it works.
# The value never goes through argv or the screen.
#
#   bash scripts/distribuir-token-ota.sh mac ubuntu windows-host
#
# Pass the ssh aliases of the OTHER machines. Windows aliases must be `windows-host`.
set -uo pipefail

TOKEN="$HOME/.config/logi/ota-token"
[ -s "$TOKEN" ] || { echo "falta $TOKEN"; exit 1; }
[ "$(stat -f '%Lp' "$TOKEN" 2>/dev/null || stat -c '%a' "$TOKEN")" = 600 ] ||
  { echo "$TOKEN precisa de chmod 600"; exit 1; }

falhou=0
for h in "$@"; do
  if [ "$h" = windows-host ]; then
    scp -q -o BatchMode=yes "$TOKEN" 'windows-host:AppData/Local/logid/ota-token' &&
      ssh -T -o BatchMode=yes windows-host 'icacls "$env:LOCALAPPDATA\logid\ota-token" /inheritance:r /grant:r "${env:USERNAME}:F" | Out-Null; & "$env:LOCALAPPDATA\logid\logi-ota.exe" --conferir | Out-String' 2>&1 |
      grep -E '^(erro|igual|novo|há|ha |build)' | cut -c1-100 || falhou=1
  else
    ssh -T -o BatchMode=yes "$h" 'umask 077; mkdir -p ~/.config/logi; cat > ~/.config/logi/ota-token' < "$TOKEN" &&
      ssh -T -o BatchMode=yes "$h" '~/.local/bin/logi-ota --conferir' 2>&1 | tail -1 | cut -c1-100 || falhou=1
  fi
  echo "  ^ $h"
done
exit "$falhou"
