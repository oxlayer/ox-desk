#!/usr/bin/env bash
# O overlay está de pé, e por qual caminho?
#
# Spec: ../specs/012-outside-the-lan.md
#
#   bash scripts/conferir-overlay.sh
#
# Read-only: não muda nada. Roda na máquina que você quer conferir, e
# faz sentido rodar dos DOIS lados -- "direto" é uma propriedade do par,
# e já vi um lado reportar direto enquanto o outro relayava.
set -uo pipefail

g=$'\033[32m'; y=$'\033[33m'; r=$'\033[31m'; d=$'\033[2m'; n=$'\033[0m'
ok(){   printf "  ${g}OK${n}    %s\n" "$1"; }
aviso(){printf "  ${y}AVISO${n} %s\n" "$1"; }
ruim(){ printf "  ${r}FALHA${n} %s\n" "$1"; }
nota(){ printf "  ${d}·     %s${n}\n" "$1"; }
sec(){  printf "\n%s\n" "$1"; }

CONF="${XDG_CONFIG_HOME:-$HOME/.config}/logi/config.toml"

sec "── Overlay ────────────────────────────────────────"
if ! command -v tailscale >/dev/null 2>&1; then
  ruim "tailscale não instalado -- veja scripts/instalar-tailscale.sh"
  exit 1
fi
ESTADO=$(tailscale status --json 2>/dev/null)
if [ -z "$ESTADO" ]; then
  ruim "o tailscaled não respondeu"
  exit 1
fi

python3 - "$ESTADO" <<'PY'
import json, sys
d = json.loads(sys.argv[1])
eu = d.get("Self") or {}
print(f"  eu: {eu.get('DNSName','?').rstrip('.')}  {', '.join(eu.get('TailscaleIPs') or [])}")
estado = d.get("BackendState", "?")
print(f"  estado: {estado}")
if estado != "Running":
    print("  (sem login? `sudo tailscale up`)")
PY

sec "── Caminho até cada par ───────────────────────────"
# `tailscale ping` diz DIRETO ou via relay, e é a única coisa que
# responde isso. O `status` sozinho mostra o ultimo caminho conhecido,
# que pode estar velho: a travessia de NAT acontece quando o trafego
# comeca, nao quando voce pergunta.
PARES=$(tailscale status --json 2>/dev/null | python3 -c '
import json,sys
d=json.load(sys.stdin)
for p in (d.get("Peer") or {}).values():
    print(p.get("DNSName","").rstrip("."))
')
if [ -z "$PARES" ]; then
  aviso "nenhum par no tailnet ainda"
else
  for p in $PARES; do
    [ -z "$p" ] && continue
    SAIDA=$(timeout 15 tailscale ping --timeout=3s --c=3 "$p" 2>&1 | tail -1)
    case "$SAIDA" in
      *direct*)  ok "$p -- $SAIDA" ;;
      *via\ DERP*|*relay*) aviso "$p -- RELAY: $SAIDA" ;;
      *)         ruim "$p -- $SAIDA" ;;
    esac
  done
  nota "relay (DERP) = NAT restritivo dos dois lados, CGNAT inclusive."
  nota "funciona, mas a tela sente: menos banda e mais latência."
fi

sec "── mDNS continua vivo? ────────────────────────────"
# A pergunta existe porque o MagicDNS mexe no resolvedor do sistema, e
# os nomes `.local` da config são como as máquinas se acham DENTRO de
# casa. Se isso quebrar, o painel marca as outras como mortas estando
# elas a um metro.
nota "nsswitch: $(grep -E '^hosts:' /etc/nsswitch.conf | sed 's/^hosts: *//')"
ACHOU=0
if [ -r "$CONF" ]; then
  while read -r nome; do
    [ -z "$nome" ] && continue
    case "$nome" in *.local) ;; *) continue ;; esac
    ACHOU=1
    if OUT=$(getent hosts "$nome" 2>/dev/null | head -1) && [ -n "$OUT" ]; then
      ok "$nome -> ${OUT%% *}"
    else
      # Não resolver pode ser a máquina dormindo, e não o DNS quebrado.
      # Dizer "FALHA no mDNS" sobre um laptop fechado mandaria você
      # consertar o que não está quebrado.
      aviso "$nome não resolve (máquina desligada, ou mDNS quebrado)"
    fi
  done < <(grep -oE '"[^"]+\.local"' "$CONF" | tr -d '"' | sort -u)
fi
[ "$ACHOU" -eq 0 ] && nota "nenhum nome .local na config"

sec "── Portas nossas, pelo overlay ────────────────────"
# A pergunta real: de fora, o que chega? SSH é TCP e o `nc -z` responde.
# A tela é QUIC/UDP e NÃO dá para sondar com conexão: UDP sem resposta é
# indistinguível de UDP filtrado. Por isso aqui só se diz o que se sabe.
for p in $PARES; do
  [ -z "$p" ] && continue
  if timeout 6 bash -c "exec 3<>/dev/tcp/$p/22" 2>/dev/null; then
    ok "$p:22 (SSH) aceita conexão"
  else
    aviso "$p:22 (SSH) não aceitou"
  fi
done
nota "9777/9778 são QUIC (UDP): sondar com conexão não prova nada."
nota "para provar a tela, abra uma de verdade -- é o único teste honesto."
