#!/usr/bin/env bash
# Builds the binaries for ONE system, checks what came out and packages it
# into dist/.
#
# It is what CI runs (specs/013), and it runs the same here: the logic of
# "which binaries exist" cannot live only in a YAML nobody runs locally.
#
#   usage:  bash scripts/build-binaries.sh <linux|windows|mac> [--sem-compilar]
#
# The binary list mirrors what `deploy.sh` installs on each machine. A new
# binary there has to be added here: `oxdesk-kvm` was already left out
# of a deploy once, built and never installed.
#
# It signs NOTHING. The Authenticode key never leaves this LAN (specs/008)
# and the Mac identity lives in a keychain over there; `deploy.sh` signs.
set -uo pipefail
cd "$(dirname "$0")/.."

ALVO="${1:?usage: build-binaries.sh <linux|windows|mac> [--sem-compilar]}"
COMPILAR=1
[ "${2:-}" = "--sem-compilar" ] && COMPILAR=0

OXDESK_BUILD="${OXDESK_BUILD:-$(git rev-parse --short HEAD)}"
OXDESK_BUILD_N="${OXDESK_BUILD_N:-$(git rev-list --count HEAD)}"
export OXDESK_BUILD OXDESK_BUILD_N

WIN=x86_64-pc-windows-gnu

# ------------------------------------------ the Mac's ffmpeg dependency
#
# The Mac is the ONLY machine that links ffmpeg: its capture is
# libavdevice's `avfoundation` and its encoder is `h264_videotoolbox`
# (specs/010). Linux and Windows capture through PipeWire/DXGI and encode
# with NVENC, loaded at runtime -- no brew dylib.
#
# And ffmpeg's soname goes INTO THE BINARY. A `oxdesk-host` built
# against major 9 (`libavcodec.63`) does not load on a machine with major
# 7 (`libavcodec.61`), and OTA refuses the whole package naming the dylib.
# That is exactly what happened: CI, on a macos-15 runner, built against
# the unversioned `ffmpeg` -- 9.0.2 today -- and the MacBook (Sonoma) has
# 7.1. The CI package was correct and uninstallable.
#
# Hence the dependency is VERSIONED on both ends, with the SAME prefix:
# `/opt/homebrew/opt/ffmpeg@7`. The unversioned `ffmpeg` does not work for
# three reasons together: it changes major on its own in a `brew upgrade`,
# it has no bottle for Sonoma (a `brew upgrade ffmpeg` on the MacBook would
# try to COMPILE 9 from source), and its prefix `/opt/homebrew/opt/ffmpeg`
# does not say what the binary was linked against.
#
# Changing major means changing both ends in the same commit. The check
# in `conferir_dependencias` breaks the build if only one moves.
FFMPEG_FORMULA="ffmpeg@7"
FFMPEG_PREFIXO="/opt/homebrew/opt/$FFMPEG_FORMULA"
FFMPEG_SONAME="libavcodec.61.dylib"
case "$ALVO" in
  linux)
    PACOTE="oxdesk-linux-x86_64"
    FORMATO='ELF 64-bit.*x86-64'
    BINARIOS="oxdesk/target/release/oxdesk
stream/target/release/oxdesk-host
stream/target/release/oxdesk-view
stream/target/release/oxdesk-bridge
stream/target/release/oxdesk-kvm
ota/target/release/oxdesk-ota"
    ;;
  windows)
    # `unsigned` in the NAME, on purpose: a raw `.exe` reaching Windows is
    # what Defender deletes the instant it is written.
    PACOTE="oxdesk-windows-x86_64-unsigned"
    FORMATO='PE32\+ executable.*x86-64'
    BINARIOS="oxdesk/target/$WIN/release/oxdesk.exe
agent/target/$WIN/release/oxdesk-agent.exe
stream/target/$WIN/release/oxdesk-host.exe
stream/target/$WIN/release/oxdesk-view.exe
stream/target/$WIN/release/oxdesk-bridge.exe
ota/target/$WIN/release/oxdesk-ota.exe"
    ;;
  mac)
    PACOTE="oxdesk-mac-arm64-unsigned"
    FORMATO='Mach-O 64-bit.*arm64'
    BINARIOS="oxdesk/target/release/oxdesk
stream/target/release/oxdesk-host
stream/target/release/oxdesk-view
stream/target/release/oxdesk-bridge
ota/target/release/oxdesk-ota"
    ;;
  *) echo "unknown target: $ALVO" >&2; exit 2 ;;
esac

compilar() {
  case "$ALVO" in
    linux)
      cargo build --release --locked --manifest-path oxdesk/Cargo.toml &&
      cargo build --release --locked --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --manifest-path stream/Cargo.toml \
        --bin oxdesk-host --bin oxdesk-view --bin oxdesk-bridge --bin oxdesk-kvm
      ;;
    windows)
      cargo build --release --locked --target "$WIN" --manifest-path oxdesk/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path agent/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --target "$WIN" --manifest-path stream/Cargo.toml \
        --bin oxdesk-host --bin oxdesk-view --bin oxdesk-bridge
      ;;
    mac)
      cargo build --release --locked --manifest-path oxdesk/Cargo.toml &&
      cargo build --release --locked --manifest-path ota/Cargo.toml &&
      cargo build --release --locked --manifest-path stream/Cargo.toml \
        --bin oxdesk-host --bin oxdesk-view --bin oxdesk-bridge
      ;;
  esac
}

echo "== $ALVO: build #$OXDESK_BUILD_N $OXDESK_BUILD =="
if [ "$COMPILAR" -eq 1 ]; then
  compilar || { echo "ERROR: the build failed"; exit 1; }
fi

falhas=0
falha() { echo "ERROR: $1"; falhas=$((falhas + 1)); }

# Dependencies the binary carries with it. Each system answers differently,
# and each answer has already been a real problem or would be one silently.
conferir_dependencias() {
  local b="$1" nome="$2" deps
  case "$ALVO" in
    linux)
      deps=$(ldd "$b" 2>&1)
      if printf '%s\n' "$deps" | grep -q 'not found'; then
        falha "$nome: missing library"; printf '%s\n' "$deps" | grep 'not found'
      fi
      ;;
    windows)
      # On that side only what ships with Windows exists: no mingw or
      # ffmpeg installed, and a missing DLL gives no warning at build time.
      deps=$(x86_64-w64-mingw32-objdump -p "$b" | grep 'DLL Name')
      if printf '%s\n' "$deps" | grep -qiE 'libgcc|libstdc|libwinpthread|avcodec|avutil|avformat'; then
        falha "$nome: depends on a DLL Windows does not have"
        printf '%s\n' "$deps" | grep -iE 'libgcc|libstdc|libwinpthread|avcodec|avutil|avformat'
      fi
      ;;
    mac)
      # oxdesk-stream links brew's ffmpeg: that is expected, and worth
      # printing, because the binary only runs with the SAME ffmpeg major
      # installed.
      deps=$(otool -L "$b")
      # `oxdesk` and `oxdesk-ota` must not depend on brew: the first is the
      # control plane, the second is what repairs the machine when the
      # rest broke. An updater that needs ffmpeg to start would be useless
      # exactly when it is needed.
      if { [ "$nome" = oxdesk ] || [ "$nome" = oxdesk-ota ]; } &&
         printf '%s\n' "$deps" | grep -q '/opt/homebrew'; then
        falha "$nome: should not depend on brew"
      fi
      # Every brew dylib has to come from the declared prefix. Coming from
      # `/opt/homebrew/opt/ffmpeg/` (unversioned) is exactly the case that
      # works on the runner and does not load on the machine.
      local fora
      fora=$(printf '%s\n' "$deps" | grep '/opt/homebrew' | grep -v "$FFMPEG_PREFIXO/" || true)
      if [ -n "$fora" ]; then
        falha "$nome: brew dylib outside $FFMPEG_PREFIXO (wrong PKG_CONFIG_PATH?)"
        printf '%s\n' "$fora"
      fi
      # The major is what OTA checks on the other side. If the runner
      # changes formula or version, this breaks HERE, at build time, and
      # not a week later in a silent OTA refusal on the Mac.
      if printf '%s\n' "$deps" | grep -q "$FFMPEG_PREFIXO" &&
         ! printf '%s\n' "$deps" | grep -q "$FFMPEG_SONAME"; then
        falha "$nome: expected $FFMPEG_SONAME ($FFMPEG_FORMULA) and did not find it -- did the major change?"
      fi
      printf '%s\n' "$deps" | grep '/opt/homebrew' | sed "s/^/  $nome:/" || true
      ;;
  esac
}

SAIDA="dist/$PACOTE-$OXDESK_BUILD_N-$OXDESK_BUILD"
rm -rf "$SAIDA" "$SAIDA.tar.gz"
mkdir -p "$SAIDA"

printf '%-24s %12s  %s\n' binary bytes sha256
LINHAS=""
while IFS= read -r b; do
  nome=$(basename "$b")
  [ -f "$b" ] || { falha "$nome does not exist at $b"; continue; }
  tipo=$(file -b "$b")
  printf '%s' "$tipo" | grep -qE "$FORMATO" || falha "$nome: unexpected format: $tipo"
  bytes=$(wc -c < "$b" | tr -d ' ')
  [ "$bytes" -gt 100000 ] || falha "$nome: only $bytes bytes"
  conferir_dependencias "$b" "$nome"
  cp "$b" "$SAIDA/" || falha "$nome: copy failed"
  soma=$(shasum -a 256 "$b" | cut -d' ' -f1)
  printf '%-24s %12s  %s\n' "$nome" "$bytes" "$soma"
  printf '%s  %s\n' "$soma" "$nome" >> "$SAIDA/SHA256SUMS"
  LINHAS="$LINHAS| \`$nome\` | $bytes | \`${soma:0:16}\` |
"
done <<< "$BINARIOS"

# The stamp proves this binary is from THIS commit. It is the same question
# the deploy asks /healthz at the end, asked before any machine receives it.
daemon="$SAIDA/oxdesk"
[ "$ALVO" = windows ] && daemon="$SAIDA/oxdesk.exe"
if [ -f "$daemon" ]; then
  grep -aqF "$OXDESK_BUILD" "$daemon" || falha "oxdesk does not carry the stamp $OXDESK_BUILD"
fi

if [ "$falhas" -ne 0 ]; then
  echo "$falhas problem(s): not packaged"
  exit 1
fi

tar -czf "$SAIDA.tar.gz" -C dist "$(basename "$SAIDA")"
pacote_bytes=$(wc -c < "$SAIDA.tar.gz" | tr -d ' ')
echo "package: $SAIDA.tar.gz ($pacote_bytes bytes)"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### $ALVO: build #$OXDESK_BUILD_N \`$OXDESK_BUILD\`"
    echo
    echo "| binary | bytes | sha256 |"
    echo "|---|---:|---|"
    printf '%s' "$LINHAS"
    echo
    echo "Package: \`$(basename "$SAIDA").tar.gz\`, $pacote_bytes bytes."
  } >> "$GITHUB_STEP_SUMMARY"
fi
