#!/usr/bin/env bash
# Opens the MacBook to the rest of the infra: inbound SSH, screen and the key.
#
# Run it ON THE MAC, in Terminal. It asks for sudo in some steps; each one
# fails on its own without taking the others down, because on a corporate
# Mac with MDM some may be blocked -- and knowing WHICH one was blocked is
# worth more than aborting on the first.
#
#   bash enable-mac.sh
set -uo pipefail

# The key to authorize comes from whoever runs this, never from the repo.
# A literal here authorized the MAINTAINER's key on the machine of anyone
# who followed the README -- a backdoor by accident, not a privacy problem.
# See .claude/rules/no-secrets-in-source.md.
PUB="${1:-$(cat ~/.ssh/id_ed25519.pub 2>/dev/null)}"
if [ -z "$PUB" ]; then
  echo "usage: bash $0 '<ssh public key>'" >&2
  echo "  or have one at ~/.ssh/id_ed25519.pub on the machine you drive from" >&2
  exit 2
fi
case "$PUB" in
  ssh-ed25519\ *|ssh-rsa\ *|ecdsa-sha2-*\ *|sk-ssh-*) ;;
  *) echo "that does not look like an SSH public key: ${PUB%% *}" >&2; exit 2 ;;
esac

ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; }
bad()  { printf '  \033[31mfailed\033[0m %s\n' "$1"; }
note() { printf '  ..    %s\n' "$1"; }

echo "== 1. Remote Login (inbound SSH) =="
if sudo systemsetup -f -setremotelogin on >/dev/null 2>&1; then
  ok "on"
else
  bad "systemsetup refused"
  note "on recent macOS it requires Full Disk Access for Terminal."
  note "manual path: Settings > General > Sharing > Remote Login"
fi
sudo systemsetup -getremotelogin 2>/dev/null | sed 's/^/        /'

echo "== 2. Firewall: do not block everything, no stealth mode =="
# Stealth mode drops ICMP and TCP with no listener WITHOUT sending RST --
# that is what made the Mac time out instead of "refused" as seen from
# Ubuntu.
FW=/usr/libexec/ApplicationFirewall/socketfilterfw
if [ -x "$FW" ]; then
  sudo "$FW" --setblockall off    >/dev/null 2>&1 && ok "blockall off"    || bad "blockall"
  sudo "$FW" --setstealthmode off >/dev/null 2>&1 && ok "stealth off"     || bad "stealth"
  sudo "$FW" --getglobalstate --getstealthmode --getblockall 2>/dev/null | sed 's/^/        /'
else
  note "socketfilterfw does not exist on this system"
fi

echo "== 3. Screen Sharing (VNC) =="
if sudo launchctl enable system/com.apple.screensharing >/dev/null 2>&1 &&
   sudo launchctl kickstart -k system/com.apple.screensharing >/dev/null 2>&1; then
  ok "service enabled"
else
  bad "launchctl refused"
  note "manual path: Settings > General > Sharing > Screen Sharing"
fi
note "ONE STEP IS MISSING IN THE UI, and it is required for ordinary clients:"
note "  Screen Sharing > (i) > Computer Settings >"
note "  tick 'VNC viewers may control screen with password' + set a password."
note "  Without it macOS requires Apple's own authentication (type 30),"
note "  which gnome-connections and most clients do not speak."

echo "== 4. Authorizing the Ubuntu key =="
mkdir -p ~/.ssh && chmod 700 ~/.ssh
touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
if grep -qF "$PUB" ~/.ssh/authorized_keys 2>/dev/null; then
  ok "key was already authorized"
else
  printf '%s\n' "$PUB" >> ~/.ssh/authorized_keys && ok "key added"
fi
note "$(wc -l < ~/.ssh/authorized_keys | tr -d ' ') key(s) in ~/.ssh/authorized_keys"

echo "== 5. What is here =="
printf '  user:    %s@%s\n' "$(whoami)" "$(hostname)"
printf '  macOS:   %s (%s)\n' "$(sw_vers -productVersion)" "$(uname -m)"
for c in brew rustc cargo git; do
  p=$(command -v "$c" 2>/dev/null)
  printf '  %-7s %s\n' "$c:" "${p:-missing}"
done

cat <<'EOF'

== done ==

Let me know. I will SSH in, install Rust if it is missing, build oxdesk
here (cross-compiling from Linux to macOS needs Apple's SDK, so the build
has to happen on this machine) and add the Mac to the three configs.

The key I use has no passphrase. If you want, put yours on the Ubuntu
box; it is the usual 'ssh-keygen -p' -- tell me and I will adjust the agent.
EOF
