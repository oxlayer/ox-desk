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

# The six binaries the setup wizard's `binaries` step looks for
# (BINS in oxdesk/src/setup_linux.rs), built the way the release builds
# them (the `linux)` case of scripts/build-binaries.sh). Installing only
# the daemon gave a newcomer a panel that reported five missing binaries,
# whose own repair advice was to run this script.
BINS=(oxdesk oxdesk-host oxdesk-view oxdesk-bridge oxdesk-kvm oxdesk-ota)

say "build dependencies"
# The same list CI installs for the Linux build (.github/workflows/
# binaries.yml): oxdesk-stream needs clang and libclang for its bindings
# and PipeWire for screen capture, on top of what the daemon needs.
missing=()
command -v cargo >/dev/null || missing+=(cargo rustc)
command -v pkg-config >/dev/null || missing+=(pkg-config)
command -v clang >/dev/null || missing+=(clang)
dpkg -s libclang-dev >/dev/null 2>&1 || missing+=(libclang-dev)
pkg-config --exists libudev 2>/dev/null || missing+=(libudev-dev)
pkg-config --exists dbus-1 2>/dev/null || missing+=(libdbus-1-dev)
pkg-config --exists libpipewire-0.3 2>/dev/null || missing+=(libpipewire-0.3-dev)
if [ ${#missing[@]} -gt 0 ]; then
  echo "missing: ${missing[*]}"
  echo "  sudo apt-get install -y ${missing[*]}"
  exit 1
fi
echo "ok: cargo, pkg-config, clang, libclang, libudev, libdbus, libpipewire"

say "building the six binaries"
cargo build --release --locked --manifest-path oxdesk/Cargo.toml
cargo build --release --locked --manifest-path ota/Cargo.toml
cargo build --release --locked --manifest-path stream/Cargo.toml \
  --bin oxdesk-host --bin oxdesk-view --bin oxdesk-bridge --bin oxdesk-kvm

say "installing them in ~/.local/bin"
# Side by side on purpose: the daemon launches its siblings next to its
# own executable, never through PATH (sys::own_bin).
installed=0
for b in "${BINS[@]}"; do
  case "$b" in
    oxdesk) src=oxdesk/target/release/$b ;;
    oxdesk-ota) src=ota/target/release/$b ;;
    *) src=stream/target/release/$b ;;
  esac
  install -D -m 755 "$src" "$HOME/.local/bin/$b"
  installed=$((installed + 1))
done
# A step that cannot say how many things it did is not a check.
[ "$installed" -eq "${#BINS[@]}" ] || { echo "installed $installed of ${#BINS[@]}"; exit 1; }
echo "ok: $installed binaries in $HOME/.local/bin"
"$HOME/.local/bin/oxdesk" --version 2>/dev/null || true

say "user service"
install -D -m 644 oxdesk/oxdesk.service "$HOME/.config/systemd/user/oxdesk.service"
systemctl --user daemon-reload
systemctl --user enable oxdesk.service
# restart, not `enable --now`: on a second run the service is already up,
# and `--now` would leave it serving the binary this script just replaced.
# Clear the port first, so an orphan the supervisor does not know about
# cannot keep answering (.claude/rules/one-supervisor-per-daemon.md).
pkill -x oxdesk 2>/dev/null || true
systemctl --user restart oxdesk.service
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

  1. open the setup wizard: http://127.0.0.1:8787/setup
     it checks every prerequisite on this machine and fixes what it can
  2. check the device NAMES (HID++ does not use Solaar's):
       cd hidpp && cargo run --release --bin probe
  3. adjust ~/.config/oxdesk/config.toml -- especially [[devices]] and each
     host's Easy-Switch channels
  4. panel at http://127.0.0.1:8787  (native GUI: cd gui && ./run.sh)
  5. diagnostics: bash status.sh

edge watching and "follow the leader" ship OFF. read
specs/005-input-switch.md before turning them on: moving a device is
irreversible on the source side.
TXT
