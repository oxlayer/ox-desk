#!/usr/bin/env bash
# Abre o MacBook para o resto da infra: SSH de entrada, tela e a chave.
#
# Rode NO MAC, no Terminal. Pede sudo em alguns passos; cada um falha
# sozinho sem derrubar os outros, porque num Mac corporativo com MDM
# alguns podem ser bloqueados -- e saber QUAL foi bloqueado vale mais que
# abortar no primeiro.
#
#   bash enable-mac.sh
set -uo pipefail

PUB='ssh-ed25519 <your-public-key> -> mac (logi)'

ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; }
nao()  { printf '  \033[31mfalhou\033[0m %s\n' "$1"; }
nota() { printf '  ..    %s\n' "$1"; }

echo "== 1. Login Remoto (SSH de entrada) =="
if sudo systemsetup -f -setremotelogin on >/dev/null 2>&1; then
  ok "ligado"
else
  nao "systemsetup recusou"
  nota "em macOS recente ele exige Acesso Total ao Disco para o Terminal."
  nota "caminho manual: Ajustes > Geral > Compartilhamento > Login Remoto"
fi
sudo systemsetup -getremotelogin 2>/dev/null | sed 's/^/        /'

echo "== 2. Firewall: nao bloquear tudo, sem modo furtivo =="
# Modo furtivo descarta ICMP e TCP sem listener SEM mandar RST -- e o que
# fazia o Mac dar timeout em vez de "recusado" visto do Ubuntu.
FW=/usr/libexec/ApplicationFirewall/socketfilterfw
if [ -x "$FW" ]; then
  sudo "$FW" --setblockall off    >/dev/null 2>&1 && ok "blockall off"    || nao "blockall"
  sudo "$FW" --setstealthmode off >/dev/null 2>&1 && ok "stealth off"     || nao "stealth"
  sudo "$FW" --getglobalstate --getstealthmode --getblockall 2>/dev/null | sed 's/^/        /'
else
  nota "socketfilterfw nao existe neste sistema"
fi

echo "== 3. Compartilhamento de Tela (VNC) =="
if sudo launchctl enable system/com.apple.screensharing >/dev/null 2>&1 &&
   sudo launchctl kickstart -k system/com.apple.screensharing >/dev/null 2>&1; then
  ok "servico habilitado"
else
  nao "launchctl recusou"
  nota "caminho manual: Ajustes > Geral > Compartilhamento > Compartilhamento de Tela"
fi
nota "FALTA UM PASSO NA INTERFACE, e ele e obrigatorio para cliente comum:"
nota "  Compartilhamento de Tela > (i) > Ajustes do Computador >"
nota "  marcar 'Visualizadores VNC podem controlar a tela com senha' + definir senha."
nota "  Sem isso o macOS exige a autenticacao propria da Apple (tipo 30),"
nota "  que o gnome-connections e a maioria dos clientes nao falam."

echo "== 4. Autorizando a chave do Ubuntu =="
mkdir -p ~/.ssh && chmod 700 ~/.ssh
touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
if grep -qF "$PUB" ~/.ssh/authorized_keys 2>/dev/null; then
  ok "chave ja estava autorizada"
else
  printf '%s\n' "$PUB" >> ~/.ssh/authorized_keys && ok "chave adicionada"
fi
nota "$(wc -l < ~/.ssh/authorized_keys | tr -d ' ') chave(s) em ~/.ssh/authorized_keys"

echo "== 5. O que esta aqui =="
printf '  usuario: %s@%s\n' "$(whoami)" "$(hostname)"
printf '  macOS:   %s (%s)\n' "$(sw_vers -productVersion)" "$(uname -m)"
for c in brew rustc cargo git; do
  p=$(command -v "$c" 2>/dev/null)
  printf '  %-7s %s\n' "$c:" "${p:-ausente}"
done

cat <<'EOF'

== pronto ==

Me avise. Eu entro por SSH, instalo o Rust se faltar, compilo o logid
aqui (cross-compile de Linux para macOS exige o SDK da Apple, entao o
build tem de acontecer nesta maquina) e ponho o Mac nas tres configs.

A chave que eu uso esta sem passphrase. Se quiser poe a sua la no Ubuntu,
e o mesmo 'ssh-keygen -p' de sempre -- me diga que eu ajusto o agent.
EOF
