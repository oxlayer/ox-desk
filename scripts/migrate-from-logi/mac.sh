#!/usr/bin/env bash
# One-time move of a Mac installation from logi/logid/logistream to oxdesk.
# Spec: specs/017-product.md ("The name: oxdesk"). Run ON the Mac.
#
#   bash scripts/migrate-from-logi/mac.sh              # before the first oxdesk deploy
#   bash scripts/migrate-from-logi/mac.sh --cleanup    # after it
#   bash scripts/migrate-from-logi/mac.sh --gui-bundle # just the app bundle's insides
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

# The INSIDE of the app bundle, which the rename of the folder did not touch:
# `mvif ~/Applications/Logi.app ~/Applications/Oxdesk.app` moved the door and
# left the nameplate, so `CFBundleIdentifier` stayed `ai.logi.gui`, the Mach-O
# stayed `Contents/MacOS/Logi`, and launchd kept labelling the running app
# `application.ai.logi.gui.<n>` -- the last place in the fleet still saying the
# old name out loud.
#
# Cheap here, and measured before doing it: this GUI asks macOS for no
# permission (no Screen Recording, no Accessibility), so changing the bundle
# id costs no re-grant, and the LaunchAgent opens the app by PATH
# (`open -a .../Oxdesk.app`), not by id, so it keeps working.
#
# ## The product name is not decoration, it is a lookup key
#
# Renaming only the main executable and `CFBundleName` got
# `FATAL: Unable to find helper app` and an app that would not start:
# Electron derives `Contents/Frameworks/<product> Helper.app` from the
# bundle's own name, so the four helpers have to travel with it. That is why
# the executable here becomes `Oxdesk` and not `oxdesk-gui` -- the Electron
# convention is executable == product name == helper prefix, and fighting it
# buys nothing.
#
# Two more things that bite, both learned the same way:
# - The signature covers Info.plist, so editing it without re-signing leaves
#   a bundle macOS refuses to launch. Nested bundles are signed before the
#   outer one.
# - `open` answers exit 0 for an app it never started, because
#   LaunchServices still has the old registration. `lsregister -f` first,
#   and then check that a process actually exists -- exit 0 is not evidence.
gui_bundle() {
  local APP=~/Applications/Oxdesk.app
  local PL="$APP/Contents/Info.plist"
  local FW="$APP/Contents/Frameworks"
  local NAME=Oxdesk
  [ -f "$PL" ] || { say "no $APP"; return 0; }

  local old
  old=$(/usr/libexec/PlistBuddy -c "Print CFBundleExecutable" "$PL" 2>/dev/null)
  if [ "$old" = "$NAME" ] && [ -d "$FW/$NAME Helper.app" ]; then
    say "the bundle already carries the new name"
    return 0
  fi

  pkill -f "Oxdesk.app/Contents" && say "closed the panel window"

  # The helpers first, inside out: each is its own bundle, with its own
  # executable and its own Info.plist.
  local h base suffix
  for h in "$FW/"*" Helper"*.app; do
    [ -d "$h" ] || continue
    base=$(basename "$h" .app)                 # e.g. "Logi Helper (GPU)"
    suffix=${base#* Helper}                    # e.g. " (GPU)", or empty
    [ "$base" = "$NAME Helper$suffix" ] && continue
    mv "$h/Contents/MacOS/$base" "$h/Contents/MacOS/$NAME Helper$suffix" 2>/dev/null
    /usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $NAME Helper$suffix" \
                            -c "Set :CFBundleName $NAME Helper$suffix" \
                            -c "Set :CFBundleIdentifier app.oxdesk.gui.helper" \
                            "$h/Contents/Info.plist" >/dev/null 2>&1
    mv "$h" "$FW/$NAME Helper$suffix.app"
    say "helper -> $NAME Helper$suffix"
  done

  [ -f "$APP/Contents/MacOS/$old" ] && mv "$APP/Contents/MacOS/$old" "$APP/Contents/MacOS/$NAME"
  /usr/libexec/PlistBuddy -c "Set :CFBundleExecutable $NAME" \
                          -c "Set :CFBundleIdentifier app.oxdesk.gui" \
                          -c "Set :CFBundleName $NAME" "$PL" >/dev/null ||
    { echo "ERROR: Info.plist"; return 1; }

  for h in "$FW/"*.app "$FW/"*.framework; do
    [ -e "$h" ] && codesign --force --sign - "$h" >/dev/null 2>&1
  done
  codesign --force --deep --sign - "$APP" 2>&1 | tail -1
  codesign -dv "$APP" 2>&1 | grep -q "Identifier=app.oxdesk.gui" ||
    { echo "ERROR: the signature did not take the new id"; return 1; }
  say "bundle -> $NAME / app.oxdesk.gui"

  local LS=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
  [ -x "$LS" ] && "$LS" -f "$APP"
  launchctl bootout "$D/app.oxdesk.gui" 2>/dev/null
  launchctl bootstrap "$D" "$LA/app.oxdesk.gui.plist" 2>/dev/null
  sleep 4
  if pgrep -f "Oxdesk.app/Contents/MacOS" >/dev/null; then
    say "panel is up again"
  else
    echo "ERROR: the panel did not come up -- see /tmp/oxdesk-gui.out"
    ("$APP/Contents/MacOS/$NAME" >/tmp/oxdesk-gui.out 2>&1 &)
    sleep 3
    head -5 /tmp/oxdesk-gui.out
    return 1
  fi
}

if [ "${1:-}" = "--gui-bundle" ]; then
  gui_bundle
  exit $?
fi

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
gui_bundle
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
