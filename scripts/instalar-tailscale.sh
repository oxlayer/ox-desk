#!/usr/bin/env bash
# Põe esta máquina no overlay, para alcançar as outras fora da LAN.
#
# Spec: ../specs/012-outside-the-lan.md
#
#   sudo bash scripts/instalar-tailscale.sh
#
# POR QUE TAILSCALE E NÃO CLOUDFLARE
#
# O transporte da nossa tela é QUIC, que é UDP (specs/010). Cloudflare
# Tunnel carrega TCP e HTTP, não isso. O Cloudflare Mesh carrega UDP,
# mas **relaya tudo pela borda da Cloudflare** -- um hop a mais em cada
# quadro, num caminho em que cada milissegundo aparece na tela. O
# Tailscale tenta conexão direta entre os pares e só cai em relay
# (DERP) quando o NAT impede.
#
# POR QUE O REPOSITÓRIO APT E NÃO O `curl | sh`
#
# O instalador oficial é um script remoto executado como root. O apt dá
# a mesma coisa assinada, auditável e atualizada pelo caminho normal da
# máquina. O Ubuntu 26.04 (resolute) tem repositório próprio.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "precisa de root: sudo bash $0"
  exit 1
fi

CODINOME="$(. /etc/os-release && echo "${VERSION_CODENAME:-}")"
if [ -z "$CODINOME" ]; then
  echo "não descobri o codinome do Ubuntu em /etc/os-release"
  exit 1
fi

if command -v tailscale >/dev/null 2>&1; then
  echo "==> tailscale já instalado: $(tailscale version | head -1)"
else
  echo "==> repositório ($CODINOME)"
  install -d -m 0755 /usr/share/keyrings
  curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODINOME}.noarmor.gpg" \
    -o /usr/share/keyrings/tailscale-archive-keyring.gpg
  curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODINOME}.tailscale-keyring.list" \
    -o /etc/apt/sources.list.d/tailscale.list
  echo "==> instalando"
  apt-get update -qq
  apt-get install -y tailscale
fi

systemctl enable --now tailscaled
echo "==> daemon: $(systemctl is-active tailscaled)"

cat <<'TXT'

==> falta o login, e ele é interativo de propósito

    sudo tailscale up

Abre uma URL para autenticar no navegador. Autenticação de máquina nova
não é coisa para um script fazer sozinho.

==> e DEPOIS, medir -- não presumir

    bash scripts/conferir-overlay.sh

Duas perguntas que só a medição responde:

1. O caminho é DIRETO ou passa por relay? Relay (DERP) acontece quando
   os dois lados estão atrás de NAT restritivo -- CGNAT de operadora é
   o caso comum no Brasil. Funciona, mas a banda e a latência pioram, e
   em tela remota isso aparece.

2. Os nomes `.local` continuam resolvendo? Nesta máquina eles NÃO
   dependem do systemd-resolved: o `/etc/nsswitch.conf` tem
   `hosts: files mdns4_minimal [NOTFOUND=return] dns`, então o Avahi
   responde ANTES de qualquer resolvedor que o Tailscale configure.
   Medido antes de instalar. Mas o custo de conferir é um comando, e o
   custo de errar é o painel marcar as outras duas máquinas como mortas
   estando elas a um metro de distância.

   Se quebrar mesmo assim:  sudo tailscale set --accept-dns=false
TXT
