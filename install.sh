#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# oxdesk setup on Ubuntu 26.04 / GNOME (Wayland).
#
# Does NOT need root: oxdesk runs as a user service and speaks HID++
# directly. The only privileged step is the /dev/hidraw ACL, and it warns
# if that is missing.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n==> %s\n' "$1"; }

say "build dependencies"
missing=()
command -v cargo >/dev/null || missing+=(cargo rustc)
pkg-config --exists libudev 2>/dev/null || missing+=(libudev-dev)
pkg-config --exists dbus-1 2>/dev/null || missing+=(libdbus-1-dev)
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}"
  echo "  sudo apt-get install -y ${missing[*]}"
  exit 1
fi
echo "ok: cargo, libudev, libdbus"

say "building oxdesk"
( cd oxdesk && cargo build --release )

say "installing the binary in ~/.local/bin"
install -D -m 755 oxdesk/target/release/oxdesk "$HOME/.local/bin/oxdesk"
"$HOME/.local/bin/oxdesk" --version 2>/dev/null || true

say "user service"
install -D -m 644 oxdesk/oxdesk.service "$HOME/.config/systemd/user/oxdesk.service"
systemctl --user daemon-reload
systemctl --user enable --now oxdesk.service
systemctl --user --no-pager status oxdesk.service | head -4

say "access to /dev/hidraw"
# Solaar's rule grants it via TAG+="uaccess", but ONLY to nodes created
# after the rule. If yours were born before it, this fixes it.
if getfacl /dev/hidraw0 2>/dev/null | grep -q "^user:$USER:"; then
  echo "ok: ACL present"
else
  echo "MISSING. Run:"
  echo "  sudo udevadm trigger --subsystem-match=hidraw --action=add"
fi

say "clipboard and files (optional)"
if ! dpkg -s gnome-shell-extension-gsconnect >/dev/null 2>&1; then
  echo "  sudo apt-get install -y gnome-shell-extension-gsconnect"
else
  gnome-extensions enable gsconnect@andyholmes.github.io 2>/dev/null \
    && echo "ok: GSConnect enabled" || echo "ok: GSConnect already enabled"
fi

cat <<'TXT'

done. next:

  1. check the device NAMES (HID++ does not use Solaar's):
       cd hidpp && cargo run --release --bin probe
  2. adjust ~/.config/oxdesk/config.toml -- especially [[devices]] and each
     host's Easy-Switch channels
  3. panel at http://127.0.0.1:8787  (native GUI: cd gui && ./run.sh)
  4. diagnostics: bash status.sh

edge watching and "follow the leader" ship OFF. read
specs/005-input-switch.md before turning them on: moving a device is
irreversible on the source side.
TXT
