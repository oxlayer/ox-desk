#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# One-time move of an Ubuntu installation from logi/logid/logistream to
# oxdesk. Spec: specs/017-product.md ("The name: oxdesk").
#
#   bash scripts/migrate-from-logi/ubuntu.sh            # before the first oxdesk deploy
#   bash scripts/migrate-from-logi/ubuntu.sh --cleanup  # after it, to remove the old binaries
#
# Idempotent: every move happens only when the new place does not exist yet
# and the old one does. Nothing is overwritten.
#
# The daemon migrates the config by itself on first start (oxdesk/src/
# legacy.rs), but the deploy needs ~/.config/oxdesk/signing to sign the
# Windows binary BEFORE any oxdesk daemon runs, so the moves happen here too.
set -u

say() { printf '  %s\n' "$*"; }
mvif() {
  if [ -e "$2" ]; then return 0; fi
  if [ -e "$1" ]; then mv "$1" "$2" && say "moved $1 -> $2"; fi
}

if [ "${1:-}" = "--cleanup" ]; then
  # Only after the new daemon answers: removing first would leave nothing.
  curl -s --max-time 3 localhost:8787/healthz >/dev/null || { echo "oxdesk is not answering; not cleaning up"; exit 1; }
  pgrep -x oxdesk >/dev/null || { echo "no oxdesk process; not cleaning up"; exit 1; }
  for b in logid logi-ota logistream-host logistream-view logistream-ponte logistream-kvm; do
    [ -e ~/.local/bin/$b ] && rm -f ~/.local/bin/$b && say "removed ~/.local/bin/$b"
  done
  rm -f ~/.local/state/logistream-host.log ~/.local/state/logistream-host.log.1 \
        ~/.local/state/logistream-host-prova.log ~/.local/state/logistream-kvm.log \
        ~/.local/state/logi-gui.log
  # A directory with the old name goes only when its replacement is really
  # there. `identidade.der` is a PRIVATE KEY and `config.toml` is a whole
  # installation's configuration -- "it was migrated" is a claim, a non-empty
  # file at the new path is the evidence. On the two remote machines the old
  # dir survived the cutover holding a copy of ANOTHER machine's config.toml,
  # which is inert only while nothing reads it: `legacy.rs` reads exactly this
  # path, so a reinstall would move a foreign config in and the daemon would
  # think it is the other machine.
  velha() { [ -s "$2" ] || { say "KEPT $1: $2 is missing"; return 0; }; [ -e "$1" ] && rm -rf "$1" && say "removed $1"; }
  velha ~/.config/logi  ~/.config/oxdesk/identity/identity.der
  velha ~/.cache/logi   ~/.cache/oxdesk/slot-names.json
  velha ~/.local/state/logi ~/.local/state/oxdesk/slots.json
  # No guard for the updater's state: `estado.json` is rewritten on the next
  # tick, and `anterior/` is a rollback copy of binaries named logid/
  # logistream-* -- nothing launches those any more, so restoring them would
  # roll back to something no supervisor starts.
  [ -e ~/.local/state/logi-ota ] && rm -rf ~/.local/state/logi-ota && say "removed ~/.local/state/logi-ota (rollback copy of the old names)"
  exit 0
fi

U=~/.config/systemd/user

echo "== stop the old daemon, updater, host and KVM"
systemctl --user disable --now logid.service logi-ota.timer 2>/dev/null
systemctl --user stop logi-ota.service 2>/dev/null
for p in logid logistream-host logistream-kvm logi-ota; do pkill -x "$p" && say "stopped $p"; done
sleep 1

echo "== config, identity and cache (same moves as oxdesk/src/legacy.rs)"
mvif ~/.config/logi ~/.config/oxdesk
mvif ~/.config/oxdesk/identidade ~/.config/oxdesk/identity
mvif ~/.config/oxdesk/identity/identidade.der ~/.config/oxdesk/identity/identity.der
mvif ~/.config/oxdesk/identity/autorizados ~/.config/oxdesk/identity/authorized
mvif ~/.cache/logi ~/.cache/oxdesk

echo "== systemd units: same content, new names"
for pair in "logid.service oxdesk.service" "logi-ota.service oxdesk-ota.service" "logi-ota.timer oxdesk-ota.timer"; do
  set -- $pair
  if [ -f "$U/$1" ] && [ ! -e "$U/$2" ]; then
    sed -e 's/logistream-kvm/oxdesk-kvm/g; s/logi-ota/oxdesk-ota/g; s/logid/oxdesk/g; s#dev/logi/#dev/oxdesk/#g' \
      "$U/$1" > "$U/$2" && rm -f "$U/$1" && say "unit $2"
  fi
done
systemctl --user daemon-reload
# Enabled now, started by the deploy: until it installs the binary, the
# unit would only restart-loop on a missing file.
systemctl --user enable oxdesk.service oxdesk-ota.timer 2>&1 | sed 's/^/  /'

echo "== the repository the deploy runs from"
mvif ~/dev/logi ~/dev/oxdesk

echo "== what is left with the old names (should be only binaries, until --cleanup)"
ls ~/.local/bin | grep -E '^(logid|logi-ota|logistream-)' | sed 's/^/  /'
ls "$U" | grep -E 'logid|logi-ota' | sed 's/^/  UNIT LEFT: /'
true
