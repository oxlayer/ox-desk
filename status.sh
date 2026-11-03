#!/usr/bin/env bash
# Diagnostico do setup Deskflow no Ubuntu/Wayland.
# Checa, em uma passada: servico, socket, servidor, portal, token, TLS,
# layouts e limites conhecidos. Read-only -- nao muda nada.
#   bash status.sh
set -uo pipefail

CONF="$HOME/.config/Deskflow/Deskflow.conf"
STATE="$HOME/.local/state/Deskflow.state"
TLSDIR="$HOME/.config/Deskflow/tls"
UNIT=deskflow-client.service

g=$'\033[32m'; y=$'\033[33m'; r=$'\033[31m'; d=$'\033[2m'; n=$'\033[0m'
ok(){   printf "  ${g}OK${n}    %s\n" "$1"; }
warn(){ printf "  ${y}AVISO${n} %s\n" "$1"; }
bad(){  printf "  ${r}FALHA${n} %s\n" "$1"; }
info(){ printf "  ${d}·     %s${n}\n" "$1"; }
sec(){  printf "\n${1}\n"; }

cfg(){ # cfg <chave> -> valor (ultima ocorrencia)
  [[ -r $CONF ]] && grep -E "^$1=" "$CONF" 2>/dev/null | tail -1 | cut -d= -f2- || true
}

REMOTE=$(cfg remoteHost)
NAME=$(cfg computerName)
MODE=$(cfg coreMode)

sec "── Sessao ─────────────────────────────────────────"
if [[ ${XDG_SESSION_TYPE:-} == wayland ]]; then
  ok "sessao Wayland (papel de cliente e o suportado no GNOME)"
else
  warn "sessao ${XDG_SESSION_TYPE:-desconhecida} -- este setup foi validado no Wayland"
fi
info "GNOME $(gnome-shell --version 2>/dev/null | awk '{print $3}'), $(. /etc/os-release; echo "$PRETTY_NAME")"

sec "── Pacote ─────────────────────────────────────────"
V=$(dpkg -s deskflow 2>/dev/null | awk '/^Version/{print $2}')
if [[ -n $V ]]; then
  ok "deskflow $V"
  info "a versao precisa casar com a do Windows (protocolo e por versao)"
else
  bad "deskflow nao instalado"
fi

sec "── Config ─────────────────────────────────────────"
[[ -n $NAME   ]] && ok "computerName = $NAME"          || bad "computerName vazio"
[[ -n $REMOTE ]] && ok "remoteHost   = $REMOTE"         || bad "remoteHost vazio -- defina o IP do servidor na GUI"
if [[ $MODE == 1 ]]; then ok "coreMode = 1 (client)"
elif [[ -z $MODE ]]; then warn "coreMode nao definido -- abra a GUI e escolha Client"
else warn "coreMode = $MODE (esperado 1 = client)"; fi
info "a tela na grade do servidor precisa se chamar exatamente '$NAME' (case-sensitive)"

sec "── Servico ────────────────────────────────────────"
if systemctl --user cat $UNIT >/dev/null 2>&1; then
  systemctl --user is-enabled --quiet $UNIT && ok "$UNIT habilitado (sobe no login)" \
                                            || warn "$UNIT nao habilitado: systemctl --user enable $UNIT"
  if systemctl --user is-active --quiet $UNIT; then
    ok "ativo, PID $(systemctl --user show -p MainPID --value $UNIT)"
  else
    bad "parado: systemctl --user start $UNIT"
  fi
  if pgrep -x deskflow >/dev/null 2>&1; then
    warn "a GUI tambem esta aberta -- instancia unica: quem subir por segundo falha"
    info "use a GUI so pra editar settings, e feche; o servico e o dono do processo"
  fi
else
  warn "unit ausente -- copie deskflow-client.service para ~/.config/systemd/user/"
fi

sec "── Rede ───────────────────────────────────────────"
if [[ -n $REMOTE ]]; then
  CONN=$(ss -tn 2>/dev/null | grep "$REMOTE:24800" | awk '{print $1}' | head -1)
  case "$CONN" in
    ESTAB) ok "conexao ESTABELECIDA com $REMOTE:24800" ;;
    "")    bad "sem conexao com $REMOTE:24800"
           if timeout 3 bash -c "echo > /dev/tcp/$REMOTE/24800" 2>/dev/null; then
             info "a porta esta ABERTA -- servidor no ar, problema e do lado do cliente"
           else
             info "porta fechada/filtrada -- servidor desligado, Deskflow fora, ou firewall do Windows"
             info "o cliente re-tenta a cada ~3s sozinho; nao precisa de poller"
           fi ;;
    *)     warn "socket em estado $CONN (teardown ou handshake em curso)" ;;
  esac
fi

sec "── Portal / Wayland ───────────────────────────────"
P=/usr/share/xdg-desktop-portal/portals/gnome.portal
if grep -q 'impl.portal.RemoteDesktop' "$P" 2>/dev/null; then
  ok "backend GNOME declara RemoteDesktop (necessario pro papel de cliente)"
else
  bad "backend GNOME nao declara RemoteDesktop -- cliente nao consegue injetar input"
fi
if grep -q 'impl.portal.InputCapture' "$P" 2>/dev/null; then
  info "InputCapture tambem declarado -- papel de SERVIDOR aqui e plausivel (nao testado)"
fi
TOK=$(grep -E '^xdpRestoreToken=' "$STATE" 2>/dev/null | cut -d= -f2-)
[[ -n $TOK ]] && ok "token do portal salvo (${TOK:0:8}…) -- sem dialogo a cada boot" \
              || warn "sem xdpRestoreToken -- o GNOME vai pedir permissao de controle remoto"

sec "── TLS ────────────────────────────────────────────"
if [[ -r $TLSDIR/deskflow.pem ]]; then
  FP=$(openssl x509 -in "$TLSDIR/deskflow.pem" -noout -fingerprint -sha256 2>/dev/null | cut -d= -f2 | tr -d ':')
  ok "certificado local presente"
  info "fingerprint daqui: ${FP:0:32}…"
else
  bad "certificado local ausente em $TLSDIR/deskflow.pem"
fi
if [[ -s $TLSDIR/trusted-servers ]]; then
  ok "$(wc -l < "$TLSDIR/trusted-servers") servidor(es) confiavel(is) registrado(s)"
  info "apagar esse arquivo faz o fingerprint ser pedido de novo (e bom: confira quando pedir)"
else
  warn "nenhum servidor confiavel ainda -- o fingerprint sera pedido na proxima conexao"
fi

sec "── Teclado ────────────────────────────────────────"
# O GNOME nao usa grupos xkb: ele TROCA o keymap pela fonte ativa. Logo o
# Deskflow so ve UMA layout -- a ativa. Ter 'br' na lista nao basta.
SRC=$(gsettings get org.gnome.desktop.input-sources sources 2>/dev/null)
ACTIVE=$(setxkbmap -query 2>/dev/null | awk '/^layout/{print $2}')
info "fontes configuradas: $SRC"
case "$ACTIVE" in
  br|pt) ok "layout ATIVA = $ACTIVE (casa com um servidor pt)" ;;
  "")    warn "nao consegui ler a layout ativa" ;;
  *)     warn "layout ATIVA = $ACTIVE -- se o servidor for pt, acentos/c-cedilha saem errados"
         info "troque com Super+Espaco, ou torne 'br' primaria:"
         info "gsettings set org.gnome.desktop.input-sources sources \"[('xkb','br'),('xkb','us')]\""
         info "se o seu teclado for fisicamente US, o certo e mudar o WINDOWS para US" ;;
esac
MISS=$(journalctl --user -u $UNIT --since '-1 day' --no-pager 2>/dev/null |
       grep -oiE 'server layouts missing on this computer: .*' | tail -1)
[[ -n $MISS ]] && warn "no log: $MISS" || true

sec "── Limites conhecidos ─────────────────────────────"
LP=$(dpkg -s libportal1 2>/dev/null | awk '/^Version/{print $2}')
if [[ -n $LP ]]; then
  if dpkg --compare-versions "$LP" lt 0.10; then
    info "libportal $LP (< 0.10): clipboard do Deskflow NAO funciona no Wayland -- use GSConnect"
  else
    ok "libportal $LP (>= 0.10): clipboard do Deskflow pode funcionar"
  fi
fi
info "Scroll Lock ligado trava o cursor na tela atual (sintoma identico a estar quebrado)"
info "bandeja nao funciona no GNOME Wayland -- por isso o core roda como servico"

sec "── Erros recentes ─────────────────────────────────"
ERR=$(journalctl --user -u $UNIT --since '-1 hour' --no-pager -p warning 2>/dev/null |
       grep -vE '^-- (No entries|Journal begins)' | tail -6)
[[ -n $ERR ]] && echo "$ERR" | sed 's/^/  /' || ok "nenhum warning na ultima hora"
echo
