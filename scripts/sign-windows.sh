#!/usr/bin/env bash
# Assina um .exe com Authenticode, daqui, antes de ele chegar ao Windows.
#
# Por que daqui e nao la: o Defender removeu o `logid.exe` cru no momento
# em que ele foi escrito, DUAS vezes -- nao deu tempo de assinar do outro
# lado. Assinar antes de copiar tira a corrida da jogada.
#
# Por que ele implicou, honestamente: executavel sem assinatura e sem
# reputacao, recem-criado, que cria tarefa agendada no login, executa
# comandos remotos e le dispositivo de entrada em modo bruto. Detectado
# como `Trojan:Win32/Bearfoos.A!ml` -- o sufixo `!ml` e palpite de modelo,
# nao assinatura de malware conhecido. A silhueta e parecida mesmo.
#
#   uso:  bash scripts/sign-windows.sh caminho/para/logid.exe
#
# A chave privada mora em ~/.config/logi/signing (nao versionada, 600).
# O certificado publico e importado nas lojas Raiz Confiavel e Editores
# Confiaveis do Windows -- vale so naquela maquina.
set -euo pipefail
export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"

EXE="${1:?uso: sign-windows.sh <arquivo.exe>}"
DIR="$HOME/.config/logi/signing"
[ -f "$DIR/key.pem" ] || { echo "sem chave em $DIR -- veja specs/008" >&2; exit 1; }

TMP="$(mktemp -u "${EXE%.exe}.assinado.XXXX.exe")"
# `-t` carimba a hora: a assinatura sobrevive ao vencimento do
# certificado, e e o que um binario normal tem. Se a TSA nao responder,
# assina sem carimbo em vez de falhar o build.
if osslsigncode sign \
      -certs "$DIR/cert.pem" -key "$DIR/key.pem" \
      -h sha256 -n "logi control plane" \
      -t http://timestamp.digicert.com \
      -in "$EXE" -out "$TMP" >/dev/null 2>&1; then
  echo "  assinado com carimbo de tempo"
else
  osslsigncode sign \
      -certs "$DIR/cert.pem" -key "$DIR/key.pem" \
      -h sha256 -n "logi control plane" \
      -in "$EXE" -out "$TMP" >/dev/null
  echo "  assinado SEM carimbo (a TSA nao respondeu)"
fi
mv "$TMP" "$EXE"
osslsigncode verify "$EXE" 2>&1 | grep -E 'Signature verification|Subject|Timestamp' | sed 's/^/  /' || true
