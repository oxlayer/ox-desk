#!/usr/bin/env bash
# Liga o ufw com as portas desta infra, e so da LAN.
#
# Rode:  sudo bash scripts/enable-ufw.sh
#
# Por que agora: o enable-sshd.sh achou o ufw inativo e seguiu em frente,
# entao as regras de escopo que ele tentou criar nunca existiram. Com
# sshd (22) e RDP do GNOME (3389) escutando em `*`, isso deixou de ser
# detalhe.
#
# CUIDADO QUE ESTE SCRIPT TOMA: mDNS.
#   Todo o desenho desta infra usa NOME, nunca IP (specs/007) -- e nome
#   aqui e `.local`, que e mDNS na 5353/udp. Um `ufw enable` com a politica
#   padrao de negar entrada mata as RESPOSTAS de mDNS, e o resultado seria
#   "nada resolve mais" com cara de problema de rede. Por isso a 5353 entra
#   explicitamente, antes do enable.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "rode com sudo: sudo bash $0" >&2
  exit 1
fi

LAN="${LAN:-192.168.1.0/24}"
echo "== LAN considerada: $LAN =="

if ! command -v ufw >/dev/null 2>&1; then
  echo "instalando ufw"
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ufw
fi

echo "== politica =="
ufw --force reset >/dev/null
ufw default deny incoming  >/dev/null
ufw default allow outgoing >/dev/null

echo "== mDNS (sem isto, nenhum nome .local resolve) =="
ufw allow in  5353/udp comment 'mDNS: nomes .local, base de specs/007' >/dev/null
ufw allow out 5353/udp comment 'mDNS'                                  >/dev/null

echo "== servicos, so da LAN =="
ufw allow from "$LAN" to any port 22   proto tcp comment 'ssh: par sonda o agente' >/dev/null
ufw allow from "$LAN" to any port 3389 proto tcp comment 'rdp: gnome-remote-desktop' >/dev/null
# KDE Connect usa a faixa 1716-1764 em TCP e UDP (descoberta + payload).
ufw allow from "$LAN" to any port 1716:1764 proto tcp comment 'kde connect' >/dev/null
ufw allow from "$LAN" to any port 1716:1764 proto udp comment 'kde connect' >/dev/null

# A API do logid NAO entra: ela escuta em 127.0.0.1 de proposito
# (specs/008). Se um dia precisar de regra aqui, algo esta errado.

echo "== ligando =="
ufw --force enable >/dev/null
ufw status verbose

cat <<EOF

== conferencia =="
EOF
echo -n "  mDNS ainda resolve o Windows?  "
getent hosts windows-host.local >/dev/null 2>&1 && echo "sim" || echo "NAO -- revise a regra 5353"
echo -n "  22 escutando?                  "
ss -tln | grep -q ':22 ' && echo "sim" || echo "nao"
echo -n "  8787 so em loopback?           "
ss -tln | grep ':8787' | grep -q '127.0.0.1' && echo "sim" || echo "revise: deveria ser so loopback"
