#!/usr/bin/env bash
# Flow-equivalent setup for Ubuntu 26.04 / GNOME 50 (Wayland)
# Run: sudo bash install.sh
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

[[ $EUID -eq 0 ]] || { echo "run with sudo: sudo bash install.sh"; exit 1; }
cd "$(dirname "$0")"

DEB=deskflow_1.27.0_amd64.deb
[[ -f $DEB ]] || { echo "missing $DEB — re-download from GitHub releases v1.27.0"; exit 1; }

echo "==> deskflow 1.27.0 (same version as the Windows .msi)"
apt-get install -y "./$DEB"

echo "==> solaar (battery + Easy-Switch channel control for MX Anywhere 3 / MX Keys)"
apt-get install -y solaar

echo "==> clipboard + file transfer between the two machines (Flow's other half)"
apt-get install -y gnome-shell-extension-gsconnect

echo "==> autostart: servico systemd de usuario para o cliente"
# Instalado no HOME do usuario que chamou o sudo, nao no root.
TARGET_HOME=$(getent passwd "${SUDO_USER:-$USER}" | cut -d: -f6)
install -o "${SUDO_USER:-$USER}" -g "${SUDO_USER:-$USER}" -D -m 644 \
  deskflow-client.service "$TARGET_HOME/.config/systemd/user/deskflow-client.service"
sudo -u "${SUDO_USER:-$USER}" XDG_RUNTIME_DIR="/run/user/$(id -u "${SUDO_USER:-$USER}")" \
  systemctl --user daemon-reload || true
sudo -u "${SUDO_USER:-$USER}" XDG_RUNTIME_DIR="/run/user/$(id -u "${SUDO_USER:-$USER}")" \
  systemctl --user enable deskflow-client.service || true

echo "==> firewall"
# Ubuntu as deskflow CLIENT only dials out; the rule below is for the
# optional reversed topology (Ubuntu as server).
ufw allow 24800/tcp comment 'deskflow (only needed if Ubuntu is the server)'
# GSConnect / KDE Connect pairing + transfers
ufw allow 1716:1764/tcp comment 'gsconnect'
ufw allow 1716:1764/udp comment 'gsconnect'
ufw reload

echo
echo "done. installed:"
deskflow --version 2>/dev/null || dpkg -s deskflow | awk '/^Version/{print "deskflow " $2}'
echo
echo "next: log out and back in once (GSConnect is a shell extension), then"
echo "follow README.md starting at 'Windows side'."
