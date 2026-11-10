#!/usr/bin/env bash
# Turns on ufw with this infra's ports, LAN only.
#
# Run:  sudo bash scripts/enable-ufw.sh
#
# Why now: enable-sshd.sh found ufw inactive and moved on, so the scoping
# rules it tried to create never existed. With sshd (22) and GNOME's RDP
# (3389) listening on `*`, that stopped being a detail.
#
# THE CARE THIS SCRIPT TAKES: mDNS.
#   This infra's whole design uses NAMES, never IPs (specs/007) -- and a
#   name here is `.local`, which is mDNS on 5353/udp. A `ufw enable` with
#   the default deny-incoming policy kills the mDNS REPLIES, and the result
#   would be "nothing resolves anymore" looking like a network problem.
#   That is why 5353 is allowed explicitly, before the enable.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "run with sudo: sudo bash $0" >&2
  exit 1
fi

# Required, with no fallback, for the reason in enable-sshd.sh.
#   ip -o -f inet addr show | awk '{print $4}'
LAN="${OXDESK_LAN_CIDR:-${LAN:-}}"
[ -n "$LAN" ] || { echo "set OXDESK_LAN_CIDR=<your.lan.0/24> (see scripts/targets.example.sh)" >&2; exit 1; }
echo "== LAN in use: $LAN =="

if ! command -v ufw >/dev/null 2>&1; then
  echo "installing ufw"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ufw
fi

echo "== policy =="
ufw --force reset >/dev/null
ufw default deny incoming  >/dev/null
ufw default allow outgoing >/dev/null

echo "== mDNS (without this, no .local name resolves) =="
ufw allow in  5353/udp comment 'mDNS: .local names, basis of specs/007' >/dev/null
ufw allow out 5353/udp comment 'mDNS'                                   >/dev/null

echo "== services, LAN only =="
ufw allow from "$LAN" to any port 22   proto tcp comment 'ssh: the peer probes the agent' >/dev/null
ufw allow from "$LAN" to any port 3389 proto tcp comment 'rdp: gnome-remote-desktop' >/dev/null
# KDE Connect uses the 1716-1764 range on TCP and UDP (discovery + payload).
ufw allow from "$LAN" to any port 1716:1764 proto tcp comment 'kde connect' >/dev/null
ufw allow from "$LAN" to any port 1716:1764 proto udp comment 'kde connect' >/dev/null

# The oxdesk API is NOT included: it listens on 127.0.0.1 on purpose
# (specs/008). If it ever needs a rule here, something is wrong.

echo "== enabling =="
ufw --force enable >/dev/null
ufw status verbose

cat <<EOF

== check ==
EOF
echo -n "  mDNS still resolves Windows?   "
if [ -n "${OXDESK_WIN_SSH:-}" ]; then
  getent hosts "${OXDESK_WIN_SSH}.local" >/dev/null 2>&1 && echo "yes" || echo "NO -- review the 5353 rule"
else
  echo "skipped: set OXDESK_WIN_SSH to check mDNS against a peer"
fi
echo -n "  22 listening?                  "
ss -tln | grep -q ':22 ' && echo "yes" || echo "no"
echo -n "  8787 only on loopback?         "
ss -tln | grep ':8787' | grep -q '127.0.0.1' && echo "yes" || echo "review: it should be loopback only"
