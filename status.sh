#!/usr/bin/env bash
# Diagnostico do setup. Read-only -- nao muda nada.
#   bash status.sh
set -uo pipefail

CONF="${XDG_CONFIG_HOME:-$HOME/.config}/logi/config.toml"
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/logi"
UNIT=logid.service
PORT=$(grep -E '^port *=' "$CONF" 2>/dev/null | head -1 | grep -oE '[0-9]+' || echo 8787)
API="http://127.0.0.1:$PORT"

g=$'\033[32m'; y=$'\033[33m'; r=$'\033[31m'; d=$'\033[2m'; n=$'\033[0m'
ok(){   printf "  ${g}OK${n}    %s\n" "$1"; }
warn(){ printf "  ${y}AVISO${n} %s\n" "$1"; }
bad(){  printf "  ${r}FALHA${n} %s\n" "$1"; }
info(){ printf "  ${d}·     %s${n}\n" "$1"; }
sec(){  printf "\n%s\n" "$1"; }

sec "── Sessao ─────────────────────────────────────────"
[ "${XDG_SESSION_TYPE:-}" = wayland ] \
  && ok "Wayland (a deteccao de borda usa org.gnome.Mutter.InputCapture)" \
  || warn "sessao ${XDG_SESSION_TYPE:-?} -- a borda foi validada no Wayland"
info "GNOME $(gnome-shell --version 2>/dev/null | awk '{print $3}'), $(. /etc/os-release; echo "$PRETTY_NAME")"

sec "── Daemon ─────────────────────────────────────────"
if systemctl --user cat $UNIT >/dev/null 2>&1; then
  systemctl --user is-enabled --quiet $UNIT && ok "$UNIT habilitado" \
    || warn "nao habilitado: systemctl --user enable $UNIT"
  systemctl --user is-active --quiet $UNIT \
    && ok "ativo, PID $(systemctl --user show -p MainPID --value $UNIT)" \
    || bad "parado: systemctl --user start $UNIT"
else
  bad "unit ausente -- rode bash install.sh"
fi
T=$(curl -s -o /dev/null -w '%{http_code} %{time_total}' --max-time 3 "$API/healthz" 2>/dev/null)
case "$T" in
  200*) ok "/healthz responde ($(echo "$T" | awk '{printf "%.0f ms", $2*1000}'))" ;;
  *)    bad "/healthz nao respondeu em $API" ;;
esac

sec "── Dispositivos (HID++ direto) ────────────────────"
if getfacl /dev/hidraw0 2>/dev/null | grep -q "^user:$USER:"; then
  ok "ACL de uaccess no /dev/hidraw"
else
  bad "sem ACL: sudo udevadm trigger --subsystem-match=hidraw --action=add"
  info "a regra do Solaar so aplica ACL a nos criados DEPOIS dela"
fi
curl -s --max-time 20 "$API/api/devices" 2>/dev/null | python3 -c '
import json,sys
try: d=json.load(sys.stdin)
except Exception: print("  (api nao respondeu)"); raise SystemExit
if d.get("error"): print(f"  erro: {d[\"error\"]}")
for rec in d.get("receivers", []):
    print(f"  {rec[\"name\"]} em {rec.get(\"hidraw\")}")
    for x in rec.get("devices", []):
        st = f"ONLINE canal {x.get(\"current_host\")}/{x.get(\"num_hosts\")} ping {x.get(\"ping_ms\")}ms" if x["online"] else "offline (outro canal)"
        print(f"     slot {x[\"slot\"]}: {x[\"name\"]:30} {st}")
' 2>/dev/null
[ -f "$STATE/slots.json" ] && info "cache de nomes: $(cat "$STATE/slots.json")"

sec "── Roteamento de input ────────────────────────────"
python3 - "$CONF" <<'PY' 2>/dev/null
import sys, tomllib
try: c = tomllib.load(open(sys.argv[1], "rb"))
except Exception as e: print(f"  config ilegivel: {e}"); raise SystemExit
b = c.get("input", {}).get("border", {})
f = c.get("input", {}).get("follow", {})
print(f"  borda:  enabled={b.get('enabled')} edge={b.get('edge')} target={b.get('target')}")
print(f"  follow: enabled={f.get('enabled')} leader={f.get('leader')} intervalo={f.get('interval_seconds')}s")
for dv in c.get("devices", []):
    print(f"  canal de {dv['name']}: {dv.get('hosts')}")
PY
N=$(journalctl --user -u $UNIT --since '-10min' --no-pager 2>/dev/null \
     | grep -cE 'border: ACTIVATED')
# Nao usar xargs aqui: `info` e funcao do shell e o xargs chamaria o
# /usr/bin/info do texinfo -- que foi o que aconteceu.
info "$N travessias de borda nos ultimos 10min"

sec "── Pares ──────────────────────────────────────────"
python3 - "$CONF" <<'PY' 2>/dev/null
import sys, tomllib, subprocess
c = tomllib.load(open(sys.argv[1], "rb"))
for h in c.get("hosts", []):
    if h.get("addr") in ("127.0.0.1", "localhost", "::1"):
        continue
    alias = h.get("ssh")
    print(f"  {h['name']} ({h['addr']})", end="")
    if not alias:
        print(" -- sem ssh na config"); continue
    r = subprocess.run(["ssh","-o","BatchMode=yes","-o","ConnectTimeout=4",
                        "-o","LogLevel=ERROR", alias, "hostname"],
                       capture_output=True, text=True)
    out = r.stdout.strip().splitlines()
    print(f" -- ssh: {out[-1] if out else 'sem resposta'}")
PY

sec "── Limites conhecidos ─────────────────────────────"
info "o GUI do Solaar compete pelo receptor -- desligado em ~/.config/autostart"
info "o Deskflow foi REMOVIDO: no modo B a troca e fisica, nao por rede"
info "mover dispositivo e irreversivel do lado de origem (specs/005)"

sec "── Erros recentes ─────────────────────────────────"
ERR=$(journalctl --user -u $UNIT --since '-1 hour' --no-pager 2>/dev/null \
      | grep -iE 'falhou|erro|error|FATAL|dividido' | tail -6)
[ -n "$ERR" ] && echo "$ERR" | sed 's/^/  /' || ok "nenhum erro na ultima hora"
echo
