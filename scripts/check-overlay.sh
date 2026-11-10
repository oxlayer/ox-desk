#!/usr/bin/env bash
# Is the overlay up, and over which path?
#
# Spec: ../specs/012-outside-the-lan.md
#
#   bash scripts/check-overlay.sh
#
# Read-only: changes nothing. Run it on the machine you want to check, and
# it makes sense to run it on BOTH sides -- "direct" is a property of the
# pair, and I have seen one side report direct while the other relayed.
set -uo pipefail

g=$'\033[32m'; y=$'\033[33m'; r=$'\033[31m'; d=$'\033[2m'; n=$'\033[0m'
ok(){   printf "  ${g}OK${n}    %s\n" "$1"; }
aviso(){ printf "  ${y}WARN${n}  %s\n" "$1"; }
bad(){ printf "  ${r}FAIL${n}  %s\n" "$1"; }
nota(){ printf "  ${d}·     %s${n}\n" "$1"; }
sec(){  printf "\n%s\n" "$1"; }

CONF="${XDG_CONFIG_HOME:-$HOME/.config}/oxdesk/config.toml"

sec "── Overlay ────────────────────────────────────────"
if ! command -v tailscale >/dev/null 2>&1; then
  bad "tailscale not installed -- see scripts/install-tailscale.sh"
  exit 1
fi
ESTADO=$(tailscale status --json 2>/dev/null)
if [ -z "$ESTADO" ]; then
  bad "tailscaled did not answer"
  exit 1
fi

python3 - "$ESTADO" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
me = d.get("Self") or {}
print(f"  me: {me.get('DNSName','?').rstrip('.')}  {', '.join(me.get('TailscaleIPs') or [])}")
state = d.get("BackendState", "?")
print(f"  state: {state}")
if state != "Running":
    print("  (not logged in? `sudo tailscale up`)")
PY

sec "── Path to each peer ──────────────────────────────"
# `tailscale ping` says DIRECT or via relay, and it is the only thing that
# answers that. `status` alone shows the last known path, which may be
# stale: NAT traversal happens when traffic starts, not when you ask.
PARES=$(tailscale status --json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
for p in (d.get("Peer") or {}).values():
    print(p.get("DNSName","").rstrip("."))
')
if [ -z "$PARES" ]; then
  aviso "no peers on the tailnet yet"
else
  for p in $PARES; do
    [ -z "$p" ] && continue
    SAIDA=$(timeout 15 tailscale ping --timeout=3s --c=3 "$p" 2>&1 | tail -1)
    case "$SAIDA" in
      *direct*)  ok "$p -- $SAIDA" ;;
      *via\ DERP*|*relay*) aviso "$p -- RELAY: $SAIDA" ;;
      *)         bad "$p -- $SAIDA" ;;
    esac
  done
  nota "relay (DERP) = restrictive NAT on both sides, CGNAT included."
  nota "it works, but the screen feels it: less bandwidth and more latency."
fi

sec "── Is mDNS still alive? ───────────────────────────"
# The question exists because MagicDNS touches the system resolver, and
# the config's `.local` names are how the machines find each other INSIDE
# the house. If that breaks, the panel marks the others as dead while
# they sit a meter away.
nota "nsswitch: $(grep -E '^hosts:' /etc/nsswitch.conf | sed 's/^hosts: *//')"
ACHOU=0
if [ -r "$CONF" ]; then
  while read -r name; do
    [ -z "$name" ] && continue
    case "$name" in *.local) ;; *) continue ;; esac
    ACHOU=1
    if OUT=$(getent hosts "$name" 2>/dev/null | head -1) && [ -n "$OUT" ]; then
      ok "$name -> ${OUT%% *}"
    else
      # Not resolving may be the machine asleep, not broken DNS. Saying
      # "mDNS FAILED" about a closed laptop would send you to fix what is
      # not broken.
      aviso "$name does not resolve (machine off, or mDNS broken)"
    fi
  done < <(grep -oE '"[^"]+\.local"' "$CONF" | tr -d '"' | sort -u)
fi
[ "$ACHOU" -eq 0 ] && nota "no .local name in the config"

sec "── Our ports, over the overlay ────────────────────"
# The real question: from outside, what gets through? SSH is TCP and
# `nc -z` answers. The screen is QUIC/UDP and CANNOT be probed with a
# connection: UDP with no reply is indistinguishable from filtered UDP.
# So here we only say what we know.
for p in $PARES; do
  [ -z "$p" ] && continue
  if timeout 6 bash -c "exec 3<>/dev/tcp/$p/22" 2>/dev/null; then
    ok "$p:22 (SSH) accepts connections"
  else
    aviso "$p:22 (SSH) did not accept"
  fi
done
nota "9777/9778 are QUIC (UDP): probing with a connection proves nothing."
nota "to prove the screen, open a real one -- it is the only honest test."
