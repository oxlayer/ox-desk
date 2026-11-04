#!/usr/bin/env bash
# Clientes de tela que de fato expoem as opcoes da GUI.
#
# Rode:  sudo bash scripts/install-clients.sh
#
# Por que: o `gnome-connections` ja instalado fala RDP e VNC, mas nao
# expoe som, microfone nem modo so-leitura por linha de comando -- e sao
# exatamente as opcoes que o painel oferece. Sem estes dois, os checks
# aparecem desabilitados (com o motivo escrito do lado, mas desabilitados).
#
#   freerdp3-x11     -> xfreerdp3: `/sound` e `/microphone`
#   tigervnc-viewer  -> vncviewer: `-ViewOnly`
#
# O daemon escolhe sozinho: a tabela em sys/linux.rs ja poe estes dois na
# frente, e o gnome-connections vira queda.
set -euo pipefail
[ "$(id -u)" -eq 0 ] || { echo "rode com sudo: sudo bash $0" >&2; exit 1; }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq freerdp3-x11 tigervnc-viewer

echo "== instalados =="
for c in xfreerdp3 vncviewer gnome-connections; do
  p=$(command -v "$c" 2>/dev/null || true)
  printf '  %-18s %s\n' "$c" "${p:-ausente}"
done
cat <<'EOF'

Recarregue o painel: os checks de som, microfone e "so olhar" devem
deixar de aparecer apagados.
EOF
