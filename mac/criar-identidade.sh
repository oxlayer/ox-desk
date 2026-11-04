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

# RODE ISTO NO TERMINAL DO PROPRIO MAC, nao por SSH.
#
# O `security import` precisa do chaveiro de LOGIN destrancado. Numa
# sessao SSH ele esta trancado e a importacao falha com "User
# interaction is not allowed" -- destrancar exigiria a senha da conta
# na linha de comando, que nao e coisa que se peca.
if [ -n "${SSH_CONNECTION:-}" ]; then
  echo "ERRO: rode este script no Terminal do Mac, nao por SSH." >&2
  echo "O chaveiro de login esta trancado em sessao remota." >&2
  exit 1
fi

# Senha temporaria para o PKCS12. Vazia faz o `security import`
# recusar; ela so existe entre o export e o import, nos dois comandos
# seguintes, e o arquivo morre com o TMP.
SENHA="logi-$$-$RANDOM"

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

# Formato LEGADO, obrigatorio.
#
# O OpenSSL 3 exporta PKCS12 com MAC SHA-256 e cifra AES, e o chaveiro
# da Apple nao le: a importacao falha com "MAC verification failed
# during PKCS12 import (wrong password?)" -- mensagem que culpa a senha
# quando o problema e o algoritmo.
openssl pkcs12 -export -out "$TMP/logi.p12" \
  -inkey "$TMP/logi.key" -in "$TMP/logi.crt" -passout pass:"$SENHA" \
  -macalg sha1 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES 2>/dev/null

# `-T /usr/bin/codesign` libera o codesign a usar a chave sem perguntar
# a senha do chaveiro a cada assinatura.
security import "$TMP/logi.p12" \
  -k "$HOME/Library/Keychains/login.keychain-db" \
  -P "$SENHA" -T /usr/bin/codesign >/dev/null

echo "pronto. identidades de assinatura:"
security find-identity -v -p codesigning 2>&1 | head -5
