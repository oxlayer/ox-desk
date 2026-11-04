#!/usr/bin/env bash
# Assina um binario com a identidade local, se ela existir.
#
# Silencioso e NAO fatal quando a identidade nao existe: a maquina
# continua funcionando com binario ad-hoc, so perde a permissao de
# tela a cada rebuild. Quem transforma isso em falha e o deploy, que
# confere o resultado -- aqui nao se derruba nada.
set -uo pipefail
NOME="logi local signing"
KC="$HOME/Library/Keychains/logi.keychain-db"
ALVO="${1:?uso: assinar.sh <caminho>}"

if [ ! -f "$KC" ]; then
  echo "sem o chaveiro $KC; rode criar-identidade.sh"
  exit 0
fi

# Destrancar sempre: e barato, e idempotente, e cobre o caso de alguem
# ter trancado o chaveiro no meio do caminho.
security unlock-keychain -p logi "$KC" 2>/dev/null || true

# `--keychain` explicito: sem ele o codesign varre a lista de busca e
# pode achar uma copia antiga no chaveiro de login, recusando por
# ambiguidade.
if codesign --force --sign "$NOME" --keychain "$KC" --timestamp=none "$ALVO" 2>&1; then
  echo "assinado: $(basename "$ALVO")"
  codesign -dv "$ALVO" 2>&1 | grep -E "Authority|Signature" | sed "s/^/  /"
else
  echo "AVISO: falhei ao assinar $(basename "$ALVO"); segue ad-hoc"
fi
