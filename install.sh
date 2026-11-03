#!/usr/bin/env bash
# Setup do logid no Ubuntu 26.04 / GNOME (Wayland).
#
# NAO precisa de root: o logid roda como servico de usuario e fala HID++
# direto. O unico passo privilegiado e a ACL do /dev/hidraw, e ele avisa
# se faltar.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n==> %s\n' "$1"; }

say "dependencias de build"
missing=()
command -v cargo >/dev/null || missing+=(cargo rustc)
pkg-config --exists libudev 2>/dev/null || missing+=(libudev-dev)
pkg-config --exists dbus-1 2>/dev/null || missing+=(libdbus-1-dev)
if [ ${#missing[@]} -gt 0 ]; then
  echo "faltam: ${missing[*]}"
  echo "  sudo apt-get install -y ${missing[*]}"
  exit 1
fi
echo "ok: cargo, libudev, libdbus"

say "compilando o logid"
( cd logid && cargo build --release )

say "instalando o binario em ~/.local/bin"
install -D -m 755 logid/target/release/logid "$HOME/.local/bin/logid"
"$HOME/.local/bin/logid" --version 2>/dev/null || true

say "servico de usuario"
install -D -m 644 logid/logid.service "$HOME/.config/systemd/user/logid.service"
systemctl --user daemon-reload
systemctl --user enable --now logid.service
systemctl --user --no-pager status logid.service | head -4

say "acesso ao /dev/hidraw"
# A regra do Solaar concede por TAG+="uaccess", mas SO a nos criados
# depois dela. Se os seus nasceram antes, isto resolve.
if getfacl /dev/hidraw0 2>/dev/null | grep -q "^user:$USER:"; then
  echo "ok: ACL presente"
else
  echo "FALTA. Rode:"
  echo "  sudo udevadm trigger --subsystem-match=hidraw --action=add"
fi

say "clipboard e arquivos (opcional)"
if ! dpkg -s gnome-shell-extension-gsconnect >/dev/null 2>&1; then
  echo "  sudo apt-get install -y gnome-shell-extension-gsconnect"
else
  gnome-extensions enable gsconnect@andyholmes.github.io 2>/dev/null \
    && echo "ok: GSConnect habilitado" || echo "ok: GSConnect ja habilitado"
fi

cat <<'TXT'

pronto. a seguir:

  1. confira os NOMES dos dispositivos (o HID++ nao usa os do Solaar):
       cd hidpp && cargo run --release --bin probe
  2. ajuste ~/.config/logi/config.toml -- em especial [[devices]] e os
     canais Easy-Switch de cada host
  3. painel em http://127.0.0.1:8787  (GUI nativa: cd gui && ./run.sh)
  4. diagnostico: bash status.sh

a vigilancia de borda e o "seguir o lider" vem DESLIGADOS. leia
specs/005-input-switch.md antes de ligar: mover dispositivo e
irreversivel do lado de origem.
TXT
