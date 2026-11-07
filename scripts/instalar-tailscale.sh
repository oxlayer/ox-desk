#!/usr/bin/env bash
# Puts this machine on the overlay, to reach the others outside the LAN.
#
# Spec: ../specs/012-outside-the-lan.md
#
#   sudo bash scripts/instalar-tailscale.sh
#
# WHY TAILSCALE AND NOT CLOUDFLARE
#
# Our screen transport is QUIC, which is UDP (specs/010). Cloudflare
# Tunnel carries TCP and HTTP, not that. Cloudflare Mesh carries UDP, but
# **relays everything through Cloudflare's edge** -- one extra hop on every
# frame, on a path where every millisecond shows on screen. Tailscale
# tries a direct connection between peers and only falls back to a relay
# (DERP) when NAT prevents it.
#
# WHY THE APT REPOSITORY AND NOT `curl | sh`
#
# The official installer is a remote script run as root. apt gives the
# same thing signed, auditable and updated through the machine's normal
# path. Ubuntu 26.04 (resolute) has its own repository.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "needs root: sudo bash $0"
  exit 1
fi

CODINOME="$(. /etc/os-release && echo "${VERSION_CODENAME:-}")"
if [ -z "$CODINOME" ]; then
  echo "could not find the Ubuntu codename in /etc/os-release"
  exit 1
fi

if command -v tailscale >/dev/null 2>&1; then
  echo "==> tailscale already installed: $(tailscale version | head -1)"
else
  echo "==> repository ($CODINOME)"
  install -d -m 0755 /usr/share/keyrings
  curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODINOME}.noarmor.gpg" \
    -o /usr/share/keyrings/tailscale-archive-keyring.gpg
  curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODINOME}.tailscale-keyring.list" \
    -o /etc/apt/sources.list.d/tailscale.list
  echo "==> installing"
  apt-get update -qq
  apt-get install -y tailscale
fi

systemctl enable --now tailscaled
echo "==> daemon: $(systemctl is-active tailscaled)"

cat <<'TXT'

==> the login is still missing, and it is interactive on purpose

    sudo tailscale up

It opens a URL to authenticate in the browser. Authenticating a new
machine is not something a script should do on its own.

==> and AFTERWARDS, measure -- do not assume

    bash scripts/conferir-overlay.sh

Two questions only measuring answers:

1. Is the path DIRECT or through a relay? A relay (DERP) happens when
   both sides are behind restrictive NAT -- carrier CGNAT is the common
   case in Brazil. It works, but bandwidth and latency get worse, and in
   a remote screen that shows.

2. Do `.local` names keep resolving? On this machine they do NOT depend
   on systemd-resolved: `/etc/nsswitch.conf` has
   `hosts: files mdns4_minimal [NOTFOUND=return] dns`, so Avahi answers
   BEFORE any resolver Tailscale configures. Measured before installing.
   But checking costs one command, and getting it wrong costs the panel
   marking the other two machines as dead while they sit a meter away.

   If it breaks anyway:  sudo tailscale set --accept-dns=false
TXT
