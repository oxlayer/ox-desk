#!/usr/bin/env bash
# Sends the OTA token from this machine to the other two, and proves it works.
# The value never goes through argv or the screen. The grep below matches
# oxdesk-ota's own output (pt-BR result words): keep it in sync with ota/.
#
#   bash scripts/distribute-ota-token.sh [extra-unix-alias ...]
#
# The targets come from scripts/targets.sh, like the deploy: the Windows
# machine is the one named there, not one whose alias we pattern-match.
# Extra aliases given as arguments are treated as unix machines.
set -uo pipefail

TOKEN="$HOME/.config/oxdesk/ota-token"
[ -s "$TOKEN" ] || { echo "missing $TOKEN"; exit 1; }
[ "$(stat -f '%Lp' "$TOKEN" 2>/dev/null || stat -c '%a' "$TOKEN")" = 600 ] ||
  { echo "$TOKEN needs chmod 600"; exit 1; }

TARGETS="$(dirname "$0")/targets.sh"
# shellcheck source=/dev/null
[ -f "$TARGETS" ] && . "$TARGETS"
WIN="${OXDESK_WIN_SSH:-}"
MAC="${OXDESK_MAC_SSH:-}"

failed=0
for h in "$@" ${MAC:+"$MAC"} ${WIN:+"$WIN"}; do
  if [ -n "$WIN" ] && [ "$h" = "$WIN" ]; then
    scp -q -o BatchMode=yes "$TOKEN" "$WIN:AppData/Local/oxdesk/ota-token" &&
      ssh -T -o BatchMode=yes "$WIN" 'icacls "$env:LOCALAPPDATA\oxdesk\ota-token" /inheritance:r /grant:r "${env:USERNAME}:F" | Out-Null; & "$env:LOCALAPPDATA\oxdesk\oxdesk-ota.exe" --conferir | Out-String' 2>&1 |
      grep -E '^(fail|igual|new|há|ha |build)' | cut -c1-100 || failed=1
  else
    ssh -T -o BatchMode=yes "$h" 'umask 077; mkdir -p ~/.config/oxdesk; cat > ~/.config/oxdesk/ota-token' < "$TOKEN" &&
      ssh -T -o BatchMode=yes "$h" '~/.local/bin/oxdesk-ota --conferir' 2>&1 | tail -1 | cut -c1-100 || failed=1
  fi
  echo "  ^ $h"
done
exit "$failed"
