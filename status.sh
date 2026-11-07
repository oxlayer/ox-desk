#!/usr/bin/env bash
# Setup diagnostics. Read-only -- changes nothing.
#   bash status.sh
set -uo pipefail

CONF="${XDG_CONFIG_HOME:-$HOME/.config}/logi/config.toml"
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/logi"
UNIT=logid.service
PORT=$(grep -E '^port *=' "$CONF" 2>/dev/null | head -1 | grep -oE '[0-9]+' || echo 8787)
API="http://127.0.0.1:$PORT"

g=$'\033[32m'; y=$'\033[33m'; r=$'\033[31m'; d=$'\033[2m'; n=$'\033[0m'
ok(){   printf "  ${g}OK${n}    %s\n" "$1"; }
warn(){ printf "  ${y}WARN${n}  %s\n" "$1"; }
bad(){  printf "  ${r}FAIL${n}  %s\n" "$1"; }
info(){ printf "  ${d}·     %s${n}\n" "$1"; }
sec(){  printf "\n%s\n" "$1"; }

sec "── Session ────────────────────────────────────────"
[ "${XDG_SESSION_TYPE:-}" = wayland ] \
  && ok "Wayland (edge detection uses org.gnome.Mutter.InputCapture)" \
  || warn "session ${XDG_SESSION_TYPE:-?} -- the edge was validated on Wayland"
info "GNOME $(gnome-shell --version 2>/dev/null | awk '{print $3}'), $(. /etc/os-release; echo "$PRETTY_NAME")"

sec "── Daemon ─────────────────────────────────────────"
if systemctl --user cat $UNIT >/dev/null 2>&1; then
  systemctl --user is-enabled --quiet $UNIT && ok "$UNIT enabled" \
    || warn "not enabled: systemctl --user enable $UNIT"
  systemctl --user is-active --quiet $UNIT \
    && ok "active, PID $(systemctl --user show -p MainPID --value $UNIT)" \
    || bad "stopped: systemctl --user start $UNIT"
else
  bad "unit missing -- run bash install.sh"
fi
T=$(curl -s -o /dev/null -w '%{http_code} %{time_total}' --max-time 3 "$API/healthz" 2>/dev/null)
case "$T" in
  200*) ok "/healthz answers ($(echo "$T" | awk '{printf "%.0f ms", $2*1000}'))" ;;
  *)    bad "/healthz did not answer at $API" ;;
esac

sec "── Devices (direct HID++) ─────────────────────────"
if getfacl /dev/hidraw0 2>/dev/null | grep -q "^user:$USER:"; then
  ok "uaccess ACL on /dev/hidraw"
else
  bad "no ACL: sudo udevadm trigger --subsystem-match=hidraw --action=add"
  info "Solaar's rule only applies the ACL to nodes created AFTER it"
fi
curl -s --max-time 20 "$API/api/devices" 2>/dev/null | python3 -c '
import json,sys
try: d=json.load(sys.stdin)
except Exception: print("  (api did not answer)"); raise SystemExit
if d.get("error"): print(f"  error: {d[\"error\"]}")
for rec in d.get("receivers", []):
    print(f"  {rec[\"name\"]} at {rec.get(\"hidraw\")}")
    for x in rec.get("devices", []):
        st = f"ONLINE channel {x.get(\"current_host\")}/{x.get(\"num_hosts\")} ping {x.get(\"ping_ms\")}ms" if x["online"] else "offline (other channel)"
        print(f"     slot {x[\"slot\"]}: {x[\"name\"]:30} {st}")
' 2>/dev/null
[ -f "$STATE/slots.json" ] && info "name cache: $(cat "$STATE/slots.json")"

sec "── Input routing ──────────────────────────────────"
python3 - "$CONF" <<'PY' 2>/dev/null
import sys, tomllib
try: c = tomllib.load(open(sys.argv[1], "rb"))
except Exception as e: print(f"  unreadable config: {e}"); raise SystemExit
b = c.get("input", {}).get("border", {})
f = c.get("input", {}).get("follow", {})
print(f"  border: enabled={b.get('enabled')} edge={b.get('edge')} target={b.get('target')}")
print(f"  follow: enabled={f.get('enabled')} leader={f.get('leader')} interval={f.get('interval_seconds')}s")
for dv in c.get("devices", []):
    print(f"  channel of {dv['name']}: {dv.get('hosts')}")
PY
N=$(journalctl --user -u $UNIT --since '-10min' --no-pager 2>/dev/null \
     | grep -cE 'border: ACTIVATED')
# Do not use xargs here: `info` is a shell function and xargs would call
# texinfo's /usr/bin/info -- which is what happened.
info "$N edge crossings in the last 10min"

sec "── Peers ──────────────────────────────────────────"
python3 - "$CONF" <<'PY' 2>/dev/null
import sys, tomllib, subprocess
c = tomllib.load(open(sys.argv[1], "rb"))
for h in c.get("hosts", []):
    if h.get("addr") in ("127.0.0.1", "localhost", "::1"):
        continue
    alias = h.get("ssh")
    print(f"  {h['name']} ({h['addr']})", end="")
    if not alias:
        print(" -- no ssh in the config"); continue
    r = subprocess.run(["ssh","-o","BatchMode=yes","-o","ConnectTimeout=4",
                        "-o","LogLevel=ERROR", alias, "hostname"],
                       capture_output=True, text=True)
    out = r.stdout.strip().splitlines()
    print(f" -- ssh: {out[-1] if out else 'no answer'}")
PY

sec "── Known limits ───────────────────────────────────"
info "the Solaar GUI competes for the receiver -- disabled in ~/.config/autostart"
info "Deskflow was REMOVED: in mode B the switch is physical, not over the network"
info "moving a device is irreversible on the source side (specs/005)"

sec "── Recent errors ──────────────────────────────────"
# The pattern matches logid's own log words, Portuguese and English: keep
# it in step with the daemon's messages.
ERR=$(journalctl --user -u $UNIT --since '-1 hour' --no-pager 2>/dev/null \
      | grep -iE 'failed|falhou|erro|error|FATAL|split state|dividido' | tail -6)
[ -n "$ERR" ] && echo "$ERR" | sed 's/^/  /' || ok "no errors in the last hour"
echo
