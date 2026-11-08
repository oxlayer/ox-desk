#!/usr/bin/env bash
# One-time move of a Mac installation from logi/logid/logistream to oxdesk.
# Spec: specs/017-product.md ("The name: oxdesk"). Run ON the Mac.
#
#   bash scripts/migrate-from-logi/mac.sh            # before the first oxdesk deploy
#   bash scripts/migrate-from-logi/mac.sh --cleanup  # after it
#
# After the deploy, macOS asks again for Screen Recording and Accessibility:
# the grant belongs to the binary, and the binaries have new names. That
# part is manual, by design of macOS.
#
# Kept on purpose: the signing keychain (logi.keychain-db, "logi local
# signing"). Renaming it means creating a new identity in the Terminal.
set -u

say() { printf '  %s\n' "$*"; }
mvif() {
  if [ -e "$2" ]; then return 0; fi
  if [ -e "$1" ]; then mv "$1" "$2" && say "moved $1 -> $2"; fi
}
LA=~/Library/LaunchAgents
D="gui/$(id -u)"

if [ "${1:-}" = "--cleanup" ]; then
  curl -s --max-time 3 localhost:8787/healthz >/dev/null || { echo "oxdesk is not answering; not cleaning up"; exit 1; }
  for b in logid logi-ota logistream-host logistream-view logistream-ponte assinar.sh criar-identidade.sh; do
    [ -e ~/.local/bin/$b ] && rm -f ~/.local/bin/$b && say "removed ~/.local/bin/$b"
  done
  rm -f ~/.local/state/logistream-host.log ~/.local/state/logistream-host.log.1 ~/.local/state/assinar.log
  # Same rule as ubuntu.sh: the old dir goes only against evidence at the new
  # path, because it holds a private key and a config.toml. Here the old dir
  # really did survive the cutover -- `mvif` skips when the destination
  # exists, and the daemon had already created the new one by itself.
  velha() { [ -s "$2" ] || { say "KEPT $1: $2 is missing"; return 0; }; [ -e "$1" ] && rm -rf "$1" && say "removed $1"; }
  velha ~/.config/logi ~/.config/oxdesk/identity/identity.der
  [ -e ~/.cache/logi ] && rm -rf ~/.cache/logi && say "removed ~/.cache/logi"
  [ -e ~/.local/state/logi-ota ] && rm -rf ~/.local/state/logi-ota && say "removed ~/.local/state/logi-ota"
  exit 0
fi

echo "== stop the old LaunchAgents and processes"
for l in logid logistream-host gui ota assinar conferir; do
  launchctl bootout "$D/ai.logi.$l" 2>/dev/null && say "booted out ai.logi.$l"
done
for p in logid logistream-host logi-ota; do pkill -x "$p" && say "stopped $p"; done
# The panel window (Electron inside Logi.app).
pkill -f "Logi.app/Contents/MacOS" && say "closed the Logi window"
sleep 1

echo "== config, identity and cache (same moves as oxdesk/src/legacy.rs)"
mvif ~/.config/logi ~/.config/oxdesk
mvif ~/.config/oxdesk/identidade ~/.config/oxdesk/identity
mvif ~/.config/oxdesk/identity/identidade.der ~/.config/oxdesk/identity/identity.der
mvif ~/.config/oxdesk/identity/autorizados ~/.config/oxdesk/identity/authorized
mvif ~/.cache/logi ~/.cache/oxdesk

echo "== app bundle, logs dir and the deploy's source copy"
mvif ~/Applications/Logi.app ~/Applications/Oxdesk.app
mkdir -p ~/Library/Logs/oxdesk
mvif ~/logi ~/oxdesk

echo "== LaunchAgents: same content, new names"
rename_plist() {
  local old="$LA/ai.logi.$1.plist" new="$LA/$2.plist"
  if [ -f "$old" ] && [ ! -e "$new" ]; then
    sed -e 's/ai\.logi\.logistream-host/app.oxdesk.host/g; s/ai\.logi\.logid/app.oxdesk.daemon/g' \
        -e 's/ai\.logi\.assinar/app.oxdesk.sign/g; s/ai\.logi\./app.oxdesk./g' \
        -e 's/logistream-host/oxdesk-host/g; s/logi-ota/oxdesk-ota/g' \
        -e 's/assinar\.sh/sign.sh/g; s/assinar\.log/sign.log/g; s/Logi\.app/Oxdesk.app/g' \
        -e 's#Logs/logid/#Logs/oxdesk/#g; s#bin/logid<#bin/oxdesk<#g' \
        "$old" > "$new" && rm -f "$old" && say "$2.plist"
  fi
}
rename_plist logid app.oxdesk.daemon
rename_plist logistream-host app.oxdesk.host
rename_plist gui app.oxdesk.gui
rename_plist ota app.oxdesk.ota
rename_plist assinar app.oxdesk.sign
# Loaded now; the daemon restart-loops until the deploy installs it, which
# then kicks it. The host and the signer start on demand.
for l in app.oxdesk.daemon app.oxdesk.host app.oxdesk.ota app.oxdesk.sign app.oxdesk.gui; do
  [ -f "$LA/$l.plist" ] && launchctl bootstrap "$D" "$LA/$l.plist" 2>/dev/null && say "loaded $l"
done

echo "== what is left with the old names"
ls "$LA" | grep 'ai\.logi\.' | sed 's/^/  PLIST LEFT: /'
ls ~/.local/bin | grep -E '^(logid|logi-ota|logistream-)' | sed 's/^/  /'
true
