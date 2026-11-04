#!/usr/bin/env bash
# Cria, uma vez, uma identidade local de assinatura de codigo no Mac.
#
# POR QUE ISTO EXISTE
#
# A permissao de Gravacao de Tela (TCC) do macOS e amarrada ao binario.
# Para um executavel **ad-hoc / linker-signed** -- que e o que o `cargo
# build` produz por padrao -- a amarracao e pelo cdhash, que muda a
# cada compilacao. Resultado medido: conceder a permissao, rodar um
# deploy, e o host voltar a travar exatamente como antes, sem erro
# nenhum no log.
#
# Assinado com uma identidade ESTAVEL, o TCC passa a amarrar no
# "designated requirement" (a identidade), e a concessao sobrevive a
# rebuilds.
#
# Idempotente: rodar de novo nao duplica nada.
set -euo pipefail

NOME="logi local signing"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if security find-certificate -c "$NOME" >/dev/null 2>&1; then
  echo "identidade '$NOME' ja existe"
  exit 0
fi

echo "criando a identidade '$NOME'..."
# `extendedKeyUsage=codeSigning` e o que faz o `codesign` aceita-la.
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$TMP/logi.key" -out "$TMP/logi.crt" \
  -subj "/CN=$NOME" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

openssl pkcs12 -export -out "$TMP/logi.p12" \
  -inkey "$TMP/logi.key" -in "$TMP/logi.crt" -passout pass: 2>/dev/null

# `-T /usr/bin/codesign` libera o codesign a usar a chave sem perguntar
# a senha do chaveiro a cada assinatura.
security import "$TMP/logi.p12" \
  -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "" -T /usr/bin/codesign >/dev/null

echo "pronto. identidades de assinatura:"
security find-identity -v -p codesigning 2>&1 | head -5
