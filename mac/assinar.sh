#!/usr/bin/env bash
# Assina um binario com a identidade local, se ela existir.
#
# Silencioso e NAO fatal quando a identidade nao existe: a maquina
# continua funcionando com binario ad-hoc, so perde a permissao de
# tela a cada rebuild. Quebrar o deploy por causa disso seria trocar
# um incomodo por uma parada.
set -uo pipefail
NOME="logi local signing"
ALVO="${1:?uso: assinar.sh <caminho>}"

if ! security find-certificate -c "$NOME" >/dev/null 2>&1; then
  echo "sem identidade '$NOME'; deixando ad-hoc (rode mac/criar-identidade.sh)"
  exit 0
fi
# `--force` porque o cargo ja deixou uma assinatura ad-hoc no binario.
if codesign --force --sign "$NOME" --timestamp=none "$ALVO" 2>&1; then
  echo "assinado: $(basename "$ALVO")"
else
  echo "AVISO: falhei ao assinar $(basename "$ALVO"); segue ad-hoc"
fi
