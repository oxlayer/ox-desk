#!/usr/bin/env bash
# Compila os binarios de UM sistema, confere o que saiu e empacota em dist/.
#
# E o que a CI roda (specs/013), e roda igual aqui: a logica de "quais
# binarios existem" nao pode morar so num YAML que ninguem executa local.
#
#   uso:  bash scripts/construir-binarios.sh <linux|windows|mac> [--sem-compilar]
#
# A lista de binarios espelha o que o `deploy.sh` instala em cada maquina.
# Binario novo la tem de entrar aqui: o `logistream-kvm` ja ficou de fora
# de um deploy uma vez, compilado e nunca instalado.
#
# NAO assina nada. A chave do Authenticode nao sai desta LAN (specs/008) e
# a identidade do Mac vive num chaveiro de la; quem assina e o `deploy.sh`.
set -uo pipefail
cd "$(dirname "$0")/.."

ALVO="${1:?uso: construir-binarios.sh <linux|windows|mac> [--sem-compilar]}"
COMPILAR=1
[ "${2:-}" = "--sem-compilar" ] && COMPILAR=0

LOGI_BUILD="${LOGI_BUILD:-$(git rev-parse --short HEAD)}"
LOGI_BUILD_N="${LOGI_BUILD_N:-$(git rev-list --count HEAD)}"
export LOGI_BUILD LOGI_BUILD_N

WIN=x86_64-pc-windows-gnu

# ----------------------------------------- a dependencia de ffmpeg do Mac
#
# O Mac e a UNICA maquina que liga no ffmpeg: a captura dele e o
# `avfoundation` do libavdevice e a codificacao e o `h264_videotoolbox`
# (specs/010). Linux e Windows capturam por PipeWire/DXGI e codificam por
# NVENC, carregado em runtime -- nenhuma dylib do brew.
#
# E o soname do ffmpeg entra NO BINARIO. Um `logistream-host` compilado
# contra o major 9 (`libavcodec.63`) nao carrega numa maquina com o major
# 7 (`libavcodec.61`), e o OTA recusa o pacote inteiro nomeando a dylib.
# Foi exatamente o que aconteceu: a CI, num runner macos-15, compilava
# contra o `ffmpeg` sem versao -- hoje 9.0.2 -- e o MacBook (Sonoma) tem
# 7.1. O pacote da CI era correto e ininstalavel.
#
# Dai a dependencia ser VERSIONADA nas duas pontas, com o MESMO prefixo:
# `/opt/homebrew/opt/ffmpeg@7`. O `ffmpeg` sem versao nao serve por tres
# razoes somadas: muda de major sozinho num `brew upgrade`, nao tem
# bottle para Sonoma (um `brew upgrade ffmpeg` no MacBook tentaria
# COMPILAR o 9 da fonte), e seu prefixo `/opt/homebrew/opt/ffmpeg` nao
# diz contra o que o binario foi ligado.
#
# Trocar de major e trocar as duas pontas no mesmo commit. A conferencia
# em `conferir_dependencias` quebra o build se so uma andar.
FFMPEG_FORMULA="ffmpeg@7"
FFMPEG_PREFIXO="/opt/homebrew/opt/$FFMPEG_FORMULA"
FFMPEG_SONAME="libavcodec.61.dylib"
case "$ALVO" in
  linux)
    PACOTE="logi-linux-x86_64"
    FORMATO='ELF 64-bit.*x86-64'
    BINARIOS="logid/target/release/logid
logistream/target/release/logistream-host
logistream/target/release/logistream-view
logistream/target/release/logistream-ponte
logistream/target/release/logistream-kvm
ota/target/release/logi-ota"
    ;;
  windows)
    # `unsigned` no NOME, de proposito: `.exe` cru chegando ao Windows e o
    # que o Defender apaga no instante da escrita.
    PACOTE="logi-windows-x86_64-unsigned"
    FORMATO='PE32\+ executable.*x86-64'
    BINARIOS="logid/target/$WIN/release/logid.exe
agent/target/$WIN/release/logid-agent.exe
logistream/target/$WIN/release/logistream-host.exe
logistream/target/$WIN/release/logistream-view.exe
ota/target/$WIN/release/logi-ota.exe"
    ;;
  mac)
    PACOTE="logi-mac-arm64-unsigned"
    FORMATO='Mach-O 64-bit.*arm64'
    BINARIOS="logid/target/release/logid
logistream/target/release/logistream-host
logistream/target/release/logistream-view
logistream/target/release/logistream-ponte
ota/target/release/logi-ota"
    ;;
  *) echo "alvo desconhecido: $ALVO" >&2; exit 2 ;;
esac

compilar() {
  case "$ALVO" in
    linux)
      cargo build --release --locked --manifest-path logid/Cargo.toml &&
      cargo build --release --locked --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --manifest-path logistream/Cargo.toml \
        --bin logistream-host --bin logistream-view --bin logistream-ponte --bin logistream-kvm
      ;;
    windows)
      cargo build --release --locked --target "$WIN" --manifest-path logid/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path agent/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path logistream/Cargo.toml \
        --bin logistream-host --bin logistream-view
      ;;
    mac)
      cargo build --release --locked --manifest-path logid/Cargo.toml &&
      cargo build --release --locked --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --manifest-path logistream/Cargo.toml \
        --bin logistream-host --bin logistream-view --bin logistream-ponte
      ;;
  esac
}

echo "== $ALVO: build #$LOGI_BUILD_N $LOGI_BUILD =="
if [ "$COMPILAR" -eq 1 ]; then
  compilar || { echo "ERRO: a compilacao falhou"; exit 1; }
fi

falhas=0
falha() { echo "ERRO: $1"; falhas=$((falhas + 1)); }

# Dependencias que o binario leva consigo. Cada sistema responde diferente,
# e cada resposta ja foi um problema real ou o seria sem aviso.
conferir_dependencias() {
  local b="$1" nome="$2" deps
  case "$ALVO" in
    linux)
      deps=$(ldd "$b" 2>&1)
      if printf '%s\n' "$deps" | grep -q 'not found'; then
        falha "$nome: biblioteca ausente"; printf '%s\n' "$deps" | grep 'not found'
      fi
      ;;
    windows)
      # Do lado de la so existe o que vem com o Windows: nao ha mingw nem
      # ffmpeg instalados, e DLL faltando nao avisa no build.
      deps=$(x86_64-w64-mingw32-objdump -p "$b" | grep 'DLL Name')
      if printf '%s\n' "$deps" | grep -qiE 'libgcc|libstdc|libwinpthread|avcodec|avutil|avformat'; then
        falha "$nome: depende de DLL que o Windows nao tem"
        printf '%s\n' "$deps" | grep -iE 'libgcc|libstdc|libwinpthread|avcodec|avutil|avformat'
      fi
      ;;
    mac)
      # O logistream liga no ffmpeg do brew: e esperado, e vale imprimir,
      # porque o binario so roda com o MESMO major do ffmpeg instalado.
      deps=$(otool -L "$b")
      # O `logid` e o `logi-ota` nao podem depender do brew: o primeiro
      # e o control plane, o segundo e quem conserta a maquina quando o
      # resto quebrou. Atualizador que precisa de ffmpeg para subir
      # perderia a serventia justamente no caso em que e necessario.
      if { [ "$nome" = logid ] || [ "$nome" = logi-ota ]; } &&
         printf '%s\n' "$deps" | grep -q '/opt/homebrew'; then
        falha "$nome: nao deveria depender do brew"
      fi
      # Toda dylib do brew tem de vir do prefixo declarado. Vir de
      # `/opt/homebrew/opt/ffmpeg/` (sem versao) e justamente o caso que
      # funciona no runner e nao carrega na maquina.
      local fora
      fora=$(printf '%s\n' "$deps" | grep '/opt/homebrew' | grep -v "$FFMPEG_PREFIXO/" || true)
      if [ -n "$fora" ]; then
        falha "$nome: dylib do brew fora de $FFMPEG_PREFIXO (PKG_CONFIG_PATH errado?)"
        printf '%s\n' "$fora"
      fi
      # O major e o que o OTA confere do outro lado. Se o runner mudar de
      # formula ou de versao, isto quebra AQUI, no build, e nao uma
      # semana depois numa recusa silenciosa do OTA no Mac.
      if printf '%s\n' "$deps" | grep -q "$FFMPEG_PREFIXO" &&
         ! printf '%s\n' "$deps" | grep -q "$FFMPEG_SONAME"; then
        falha "$nome: esperava $FFMPEG_SONAME ($FFMPEG_FORMULA) e nao achei -- o major mudou?"
      fi
      printf '%s\n' "$deps" | grep '/opt/homebrew' | sed "s/^/  $nome:/" || true
      ;;
  esac
}

SAIDA="dist/$PACOTE-$LOGI_BUILD_N-$LOGI_BUILD"
rm -rf "$SAIDA" "$SAIDA.tar.gz"
mkdir -p "$SAIDA"

printf '%-24s %12s  %s\n' binario bytes sha256
LINHAS=""
while IFS= read -r b; do
  nome=$(basename "$b")
  [ -f "$b" ] || { falha "$nome nao existe em $b"; continue; }
  tipo=$(file -b "$b")
  printf '%s' "$tipo" | grep -qE "$FORMATO" || falha "$nome: formato inesperado: $tipo"
  bytes=$(wc -c < "$b" | tr -d ' ')
  [ "$bytes" -gt 100000 ] || falha "$nome: so $bytes bytes"
  conferir_dependencias "$b" "$nome"
  cp "$b" "$SAIDA/" || falha "$nome: nao copiou"
  soma=$(shasum -a 256 "$b" | cut -d' ' -f1)
  printf '%-24s %12s  %s\n' "$nome" "$bytes" "$soma"
  printf '%s  %s\n' "$soma" "$nome" >> "$SAIDA/SHA256SUMS"
  LINHAS="$LINHAS| \`$nome\` | $bytes | \`${soma:0:16}\` |
"
done <<< "$BINARIOS"

# O carimbo prova que este binario e DESTE commit. E a mesma pergunta que o
# deploy faz ao /healthz no fim, feita antes de qualquer maquina receber.
daemon="$SAIDA/logid"
[ "$ALVO" = windows ] && daemon="$SAIDA/logid.exe"
if [ -f "$daemon" ]; then
  grep -aqF "$LOGI_BUILD" "$daemon" || falha "logid nao carrega o carimbo $LOGI_BUILD"
fi

if [ "$falhas" -ne 0 ]; then
  echo "$falhas problema(s): nao empacotei"
  exit 1
fi

tar -czf "$SAIDA.tar.gz" -C dist "$(basename "$SAIDA")"
pacote_bytes=$(wc -c < "$SAIDA.tar.gz" | tr -d ' ')
echo "pacote: $SAIDA.tar.gz ($pacote_bytes bytes)"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### $ALVO: build #$LOGI_BUILD_N \`$LOGI_BUILD\`"
    echo
    echo "| binario | bytes | sha256 |"
    echo "|---|---:|---|"
    printf '%s' "$LINHAS"
    echo
    echo "Pacote: \`$(basename "$SAIDA").tar.gz\`, $pacote_bytes bytes."
  } >> "$GITHUB_STEP_SUMMARY"
fi
