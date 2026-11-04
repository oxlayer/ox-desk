#!/usr/bin/env bash
# Cria, uma vez, a identidade local de assinatura de codigo no Mac.
#
# POR QUE ISTO EXISTE
#
# A permissao de Gravacao de Tela (TCC) e amarrada ao binario. Para um
# executavel ad-hoc / linker-signed -- o que o `cargo build` produz --
# a amarracao e pelo cdhash, que muda a cada compilacao. Medido:
# conceder a permissao, rodar um deploy, e o host voltar a travar
# exatamente como antes, sem erro nenhum no log.
#
# Com identidade ESTAVEL o TCC amarra no designated requirement, e a
# concessao sobrevive a rebuilds.
#
# POR QUE UM CHAVEIRO PROPRIO, E NAO O DE LOGIN
#
# No chaveiro de login o `codesign` pede autorizacao por dialogo
# grafico a cada assinatura -- e por SSH nem dialogo ha: falha com
# `errSecInternalComponent`. Digitar a senha no dialogo resolve UMA
# operacao; nao fixa nada.
#
# Um chaveiro dedicado, com senha que nos conhecemos, pode ser
# destrancado e ter a lista de particao ajustada por linha de comando
# -- e ai o `codesign` roda sem perguntar, inclusive por SSH. E o
# padrao que qualquer CI de macOS usa.
#
# A senha nao e segredo de proposito: o chaveiro guarda UMA chave
# auto-assinada cujo unico poder e assinar os nossos proprios binarios
# para o TCC os reconhecer. Nao assina nada que outra maquina confie.
set -euo pipefail

NOME="logi local signing"
KC="$HOME/Library/Keychains/logi.keychain-db"
KCPASS="logi"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Tirar do chaveiro de LOGIN, se uma tentativa anterior deixou la: duas
# identidades com o mesmo nome fazem o codesign recusar por ambiguidade.
while security find-certificate -c "$NOME" "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1; do
  security delete-certificate -c "$NOME" "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1 || break
done

# IDEMPOTENTE, e isto importa mais do que parece.
#
# Recriar a identidade gera um certificado NOVO, com chave publica
# nova -- e o designated requirement muda junto. O TCC trata isso como
# outro programa e DESCARTA a concessao de Gravacao de Tela, que so o
# dono da maquina pode dar de volta, a mao, na interface.
#
# Ou seja: rodar este script duas vezes custa uma ida as Configuracoes.
# Por isso ele recusa, em vez de "garantir estado".
if [ -f "$KC" ] && security find-certificate -c "$NOME" "$KC" >/dev/null 2>&1; then
  echo "a identidade '$NOME' ja existe em $KC -- nada a fazer."
  echo "Recria-la invalidaria a permissao de Gravacao de Tela ja concedida."
  echo "Para recriar mesmo assim: security delete-keychain \"$KC\" e rode de novo."
  security find-identity -p codesigning "$KC" 2>&1 | tail -2
  exit 0
fi

security delete-keychain "$KC" 2>/dev/null || true
security create-keychain -p "$KCPASS" "$KC"
# Sem auto-lock: um chaveiro que tranca sozinho volta a pedir dialogo
# no meio de um deploy.
security set-keychain-settings -lut 100000 "$KC"
security unlock-keychain -p "$KCPASS" "$KC"

SENHA="tmp-$$-$RANDOM"
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -keyout "$TMP/k.pem" -out "$TMP/c.pem" -subj "/CN=$NOME" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

# Formato LEGADO: o OpenSSL 3+ exporta PKCS12 com MAC SHA-256, que o
# chaveiro da Apple nao le -- e o erro culpa a senha ("MAC verification
# failed, wrong password?") quando o problema e o algoritmo.
openssl pkcs12 -export -out "$TMP/p.p12" -inkey "$TMP/k.pem" -in "$TMP/c.pem" \
  -passout pass:"$SENHA" -macalg sha1 -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES 2>/dev/null

security import "$TMP/p.p12" -k "$KC" -P "$SENHA" -T /usr/bin/codesign -A

# ISTO e o que elimina o dialogo. Sem a lista de particao o macOS pede
# autorizacao grafica a cada uso da chave privada.
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KCPASS" "$KC" >/dev/null

# Entrar na lista de busca sem expulsar os que ja estao.
ATUAIS=$(security list-keychains -d user | sed 's/[" ]//g' | tr '\n' ' ')
# shellcheck disable=SC2086
security list-keychains -d user -s $ATUAIS "$KC"

echo "identidade criada em $KC"
security find-identity -p codesigning "$KC" 2>&1 | tail -3
