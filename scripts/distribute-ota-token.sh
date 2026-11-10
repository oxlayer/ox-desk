#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Sends the OTA token from this machine to the other two, and proves it works.
# The value never goes through argv or the screen.
#
# It shows the LAST line of `oxdesk-ota --check` and does not filter by
# word. There used to be a `grep -E '^(erro|igual|novo|...)'` here with a
# comment saying to keep it in sync with `ota/` -- and measured on
# 2026-11-10 it did not match the normal output at all, which begins
# `mais-novo-aqui:`. So it hid the very state it was meant to report. A
# filter that has to track another program's vocabulary is a coupling that
# breaks in silence; the last line is enough and never goes stale.
#
# On the Windows side `Out-String` ends with a blank CRLF line, so a plain
# `tail -1` reported NOTHING for that machine -- an empty line under
# `^ <alias>`, which reads like "no answer" and was really "last line is
# the empty one". Hence the `tr -d '\r'` and the blank-line drop.
#
#   bash scripts/distribute-ota-token.sh [extra-unix-alias ...]
#
# The targets come from scripts/targets.sh, like the deploy: the Windows
# machine is the one named there, not one whose alias we pattern-match.
# Extra aliases given as arguments are treated as unix machines.
set -uo pipefail

TOKEN="$HOME/.config/oxdesk/ota-token"
[ -s "$TOKEN" ] || { echo "missing $TOKEN"; exit 1; }
# GNU `stat` FIRST, BSD second. Written the other way round, this check
# could never pass on Linux: `stat -f '%Lp'` there means "file system
# status", prints the whole filesystem block to STDOUT and only then exits
# 1, so the `||` fallback appended `600` to that dump and the comparison
# was against several lines of ext4 trivia. Measured 2026-11-10 on a token
# that was already 600.
perms=$(stat -c '%a' "$TOKEN" 2>/dev/null || stat -f '%Lp' "$TOKEN" 2>/dev/null)
# It says what it found: "needs chmod 600" on a file that already was 600
# sent me looking at the file instead of at the check.
[ "$perms" = 600 ] ||
  { echo "$TOKEN needs chmod 600 (it is ${perms:-unreadable})"; exit 1; }

TARGETS="$(dirname "$0")/targets.sh"
# shellcheck source=/dev/null
[ -f "$TARGETS" ] && . "$TARGETS"
WIN="${OXDESK_WIN_SSH:-}"
MAC="${OXDESK_MAC_SSH:-}"

failed=0
for h in "$@" ${MAC:+"$MAC"} ${WIN:+"$WIN"}; do
  if [ -n "$WIN" ] && [ "$h" = "$WIN" ]; then
    scp -q -o BatchMode=yes "$TOKEN" "$WIN:AppData/Local/oxdesk/ota-token" &&
      ssh -T -o BatchMode=yes "$WIN" 'icacls "$env:LOCALAPPDATA\oxdesk\ota-token" /inheritance:r /grant:r "${env:USERNAME}:F" | Out-Null; & "$env:LOCALAPPDATA\oxdesk\oxdesk-ota.exe" --check | Out-String' 2>&1 |
      tr -d '\r' | sed '/^[[:space:]]*$/d' | tail -1 | cut -c1-100 || failed=1
  else
    ssh -T -o BatchMode=yes "$h" 'umask 077; mkdir -p ~/.config/oxdesk; cat > ~/.config/oxdesk/ota-token' < "$TOKEN" &&
      # `--check`, falling back to `--conferir`: the flag was renamed, an
      # unknown argument exits 2, and the peer runs its own build -- the
      # Mac cannot be deployed while it has no screen. The fallback
      # disappears on its own once every machine is current.
      ssh -T -o BatchMode=yes "$h" \
        '~/.local/bin/oxdesk-ota --check || ~/.local/bin/oxdesk-ota --conferir' 2>&1 |
        tail -1 | cut -c1-100 || failed=1
  fi
  echo "  ^ $h"
done
exit "$failed"
