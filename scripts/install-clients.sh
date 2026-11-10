#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Screen clients that actually expose the GUI's options.
#
# Run:  sudo bash scripts/install-clients.sh
#
# Why: the already-installed `gnome-connections` speaks RDP and VNC, but
# does not expose sound, microphone or view-only mode on the command line
# -- and those are exactly the options the panel offers. Without these
# two, the checkboxes show up disabled (with the reason written next to
# them, but disabled).
#
#   freerdp3-x11     -> xfreerdp3: `/sound` and `/microphone`
#   tigervnc-viewer  -> vncviewer: `-ViewOnly`
#
# The daemon picks on its own: the table in sys/linux.rs already puts
# these two first, and gnome-connections becomes the fallback.
set -euo pipefail
[ "$(id -u)" -eq 0 ] || { echo "run with sudo: sudo bash $0" >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq freerdp3-x11 tigervnc-viewer

echo "== installed =="
for c in xfreerdp3 vncviewer gnome-connections; do
  p=$(command -v "$c" 2>/dev/null || true)
  printf '  %-18s %s\n' "$c" "${p:-missing}"
done
cat <<'EOF'

Reload the panel: the sound, microphone and "so olhar" (view only)
checkboxes should no longer appear greyed out.
EOF
