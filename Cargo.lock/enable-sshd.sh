#!/usr/bin/env bash
# Habilita SSH de entrada no Ubuntu e instala o cliente de tela remota.
#
# Por que precisa de voce: as duas coisas pedem `sudo`, e sudo aqui pede
# senha. Tudo o que NAO pede senha eu ja fiz.
#
# O que isto desbloqueia:
#   1. A borda do daemon no Windows passar a trocar pelo `focus`, com
#      dead-man's switch VERIFICADO no alvo -- aposentando o `watch-edge`,
#      que funciona mas nao passa pelos gates (specs/001, specs/005).
#   2. O file browser olhar nos dois sentidos, nao so daqui para la
#      (specs/003).
#   3. Eu conseguir instalar e compilar no Mac quando ele entrar.
#
# Rode:  sudo bash scripts/enable-sshd.sh
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "rode com sudo: sudo bash $0" >&2
  exit 1
fi

ALVO_USER="${SUDO_USER:-user}"
LAN="192.168.1.0/24"

echo "== 1. instalando openssh-server e cliente de tela =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# gnome-connections fala RDP *e* VNC: um app cobre o Windows (RDP) e o
# Mac (VNC), em vez de dois clientes separados.
apt-get install -y -qq openssh-server gnome-connections

echo "== 2. endurecendo o sshd =="
# Arquivo proprio em sshd_config.d: nao editamos o sshd_config distribuido,
# entao um upgrade do pacote nao conflita com a nossa escolha.
install -d -m 0755 /etc/ssh/sshd_config.d
cat > /etc/ssh/sshd_config.d/10-logi.conf <<EOF
# Gerado por scripts/enable-sshd.sh (repo logi).
#
# Somente chave. Senha desligada de proposito: esta maquina fica numa LAN
# domestica com dispositivos que nao sao meus, e senha e o que um scanner
# tenta primeiro.
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
PermitRootLogin no

# Só este usuario. Um AllowUsers explicito e mais barato de auditar do que
# lembrar quais contas de sistema tem shell.
AllowUsers $ALVO_USER

# Sem encaminhamento: nada aqui precisa, e X11/agent forwarding sao as
# duas pontas por onde um host comprometido alcanca de volta.
X11Forwarding no
AllowAgentForwarding no
AllowTcpForwarding yes
EOF
sshd -t && echo "  config valida"

echo "== 3. autorizando a chave que ja existe (se houver) =="
HOME_DIR=$(getent passwd "$ALVO_USER" | cut -d: -f6)
install -d -m 0700 -o "$ALVO_USER" -g "$ALVO_USER" "$HOME_DIR/.ssh"
touch "$HOME_DIR/.ssh/authorized_keys"
chmod 600 "$HOME_DIR/.ssh/authorized_keys"
chown "$ALVO_USER:$ALVO_USER" "$HOME_DIR/.ssh/authorized_keys"
echo "  $HOME_DIR/.ssh/authorized_keys pronto ($(wc -l < "$HOME_DIR/.ssh/authorized_keys") chave(s))"

echo "== 4. firewall: 22 e 3389 so da LAN =="
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q '^Status: active'; then
  ufw allow from "$LAN" to any port 22 proto tcp comment 'logi: ssh LAN' >/dev/null
  ufw allow from "$LAN" to any port 3389 proto tcp comment 'logi: rdp LAN' >/dev/null
  echo "  regras ufw adicionadas"
else
  echo "  ufw inativo -- nada a fazer (sem firewall local bloqueando)"
fi

echo "== 5. ligando =="
systemctl enable --now ssh
systemctl is-active ssh
ss -tln | grep -E ':22\b' || echo "  AVISO: nada escutando na 22"

cat <<EOF

== pronto ==

Escutando na 22, somente chave, somente o usuario $ALVO_USER.

Me avise que eu faco o resto: gerar a chave no Windows, autorizar aqui,
religar a borda do daemon pelo focus e ligar o file browser nos dois
sentidos. A passphrase da chave fica com voce -- eu gero sem, voce roda
um 'ssh-keygen -p' depois, igual fizemos na chave de ida.
EOF
