#!/usr/bin/env bash
# Builds the binaries for ONE system, checks what came out and packages it
# into dist/.
#
# It is what CI runs (specs/013), and it runs the same here: the logic of
# "which binaries exist" cannot live only in a YAML nobody runs locally.
#
#   usage:  bash scripts/build-binaries.sh <linux|windows|mac> [--no-build]
#
# The binary list mirrors what `deploy.sh` installs on each machine. A new
# binary there has to be added here: `oxdesk-kvm` was already left out
# of a deploy once, built and never installed.
#
# It signs NOTHING. The Authenticode key never leaves this LAN (specs/008)
# and the Mac identity lives in a keychain over there; `deploy.sh` signs.
set -uo pipefail
cd "$(dirname "$0")/.."

TARGET="${1:?usage: build-binaries.sh <linux|windows|mac> [--no-build]}"
BUILD=1
[ "${2:-}" = "--no-build" ] && BUILD=0

OXDESK_BUILD="${OXDESK_BUILD:-$(git rev-parse --short HEAD)}"
# BUILD_BASE + the commit count: the squash of 2026-11-08 reset the count,
# and a package whose `n` went backwards makes the updater downgrade every
# machine. See the comment in deploy.sh.
OXDESK_BUILD_N="${OXDESK_BUILD_N:-$(( $(cat BUILD_BASE 2>/dev/null || echo 0) + $(git rev-list --count HEAD) ))}"
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
FFMPEG_PREFIX="/opt/homebrew/opt/$FFMPEG_FORMULA"
FFMPEG_SONAME="libavcodec.61.dylib"
# The script CHECKS this prefix, so it also SETS it: relying on the caller
# leaves `bash scripts/build-binaries.sh mac` by hand behaving differently
# from CI, and the header of this file exists because the packaging logic
# cannot live only in a YAML nobody runs locally.
if [ "$TARGET" = mac ]; then
  export PKG_CONFIG_PATH="$FFMPEG_PREFIX/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi

case "$TARGET" in
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
  *) echo "unknown target: $TARGET" >&2; exit 2 ;;
esac

# The MacBook cannot hold the `ffmpeg@7` keg: macOS 14 is Tier 3 for
# Homebrew, there is no bottle, and building it from source fails there
# (measured: `make: *** [frameworkinstallapps] Error 2`). So on that machine
# `opt/ffmpeg@7` is an ALIAS to the same keg as `opt/ffmpeg`, the dylibs
# carry the UNVERSIONED install_name, and every binary built there records
# it -- no PKG_CONFIG_PATH changes that, because the path comes from the
# dylib and not from pkg-config. `deploy.sh` documents the same thing and
# tolerates it, because for a binary that stays on THAT machine both paths
# resolve.
#
# A package is different: it has to pin the versioned prefix, which is what
# makes it installable on a machine that has only the keg -- a CI runner, or
# the next Mac. So the recorded paths are rewritten on the COPY that goes
# into the package, and only after measuring that both prefixes are the SAME
# FILE. If they ever stop being, this refuses: a package that lies about
# what it was linked against is worse than one that fails to build.
fixar_prefixo_ffmpeg() {
  [ "$TARGET" = mac ] || return 0
  local f="$1" name="$2" d new a b
  # Both prefixes come from the ONE declaration: `@7` is the declared one
  # and `${FFMPEG_PREFIX%@*}` is the same formula unversioned. Deriving
  # them beats writing the paths twice, and beats a substitution with
  # escaped slashes -- which is what broke here first, putting a literal
  # `opt\/ffmpeg@7\/` in the message.
  local bare="${FFMPEG_PREFIX%@*}"
  for d in $(otool -L "$f" | grep -o "$bare/[^ ]*"); do
    new="$FFMPEG_PREFIX/${d#"$bare"/}"
    a=$(stat -f %i "$d" 2>/dev/null)
    b=$(stat -f %i "$new" 2>/dev/null)
    if [ -z "$a" ] || [ -z "$b" ]; then
      falha "$name: $new does not exist -- install the $FFMPEG_FORMULA keg, or fix the alias"
      return 1
    fi
    if [ "$a" != "$b" ]; then
      falha "$name: $d and $new are DIFFERENT files; not rewriting the package's linkage"
      return 1
    fi
    install_name_tool -change "$d" "$new" "$f" || { falha "$name: install_name_tool on $d"; return 1; }
    echo "  $name: $d -> $new (same keg, inode $a)"
  done
}

compilar() {
  case "$TARGET" in
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

echo "== $TARGET: build #$OXDESK_BUILD_N $OXDESK_BUILD =="
if [ "$BUILD" -eq 1 ]; then
  compilar || { echo "ERROR: the build failed"; exit 1; }
fi

failures=0
falha() { echo "ERROR: $1"; failures=$((failures + 1)); }

# Dependencies the binary carries with it. Each system answers differently,
# and each answer has already been a real problem or would be one silently.
conferir_dependencias() {
  local b="$1" name="$2" deps
  case "$TARGET" in
    linux)
      deps=$(ldd "$b" 2>&1)
      if printf '%s\n' "$deps" | grep -q 'not found'; then
        falha "$name: missing library"; printf '%s\n' "$deps" | grep 'not found'
      fi
      ;;
    windows)
      # On that side only what ships with Windows exists: no mingw or
      # ffmpeg installed, and a missing DLL gives no warning at build time.
      deps=$(x86_64-w64-mingw32-objdump -p "$b" | grep 'DLL Name')
      if printf '%s\n' "$deps" | grep -qiE 'libgcc|libstdc|libwinpthread|avcodec|avutil|avformat'; then
        falha "$name: depends on a DLL Windows does not have"
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
      if { [ "$name" = oxdesk ] || [ "$name" = oxdesk-ota ]; } &&
         printf '%s\n' "$deps" | grep -q '/opt/homebrew'; then
        falha "$name: should not depend on brew"
      fi
      # Every brew dylib has to come from the declared prefix. Coming from
      # `/opt/homebrew/opt/ffmpeg/` (unversioned) is exactly the case that
      # works on the runner and does not load on the machine.
      local fora
      fora=$(printf '%s\n' "$deps" | grep '/opt/homebrew' | grep -v "$FFMPEG_PREFIX/" || true)
      if [ -n "$fora" ]; then
        falha "$name: brew dylib outside $FFMPEG_PREFIX (wrong PKG_CONFIG_PATH?)"
        printf '%s\n' "$fora"
      fi
      # The major is what OTA checks on the other side. If the runner
      # changes formula or version, this breaks HERE, at build time, and
      # not a week later in a silent OTA refusal on the Mac.
      if printf '%s\n' "$deps" | grep -q "$FFMPEG_PREFIX" &&
         ! printf '%s\n' "$deps" | grep -q "$FFMPEG_SONAME"; then
        falha "$name: expected $FFMPEG_SONAME ($FFMPEG_FORMULA) and did not find it -- did the major change?"
      fi
      printf '%s\n' "$deps" | grep '/opt/homebrew' | sed "s/^/  $name:/" || true
      ;;
  esac
}

OUTDIR="dist/$PACOTE-$OXDESK_BUILD_N-$OXDESK_BUILD"
rm -rf "$OUTDIR" "$OUTDIR.tar.gz"
mkdir -p "$OUTDIR"

printf '%-24s %12s  %s\n' binary bytes sha256
ROWS=""
while IFS= read -r b; do
  name=$(basename "$b")
  [ -f "$b" ] || { falha "$name does not exist at $b"; continue; }
  # Copy FIRST, then check: `fixar_prefixo_ffmpeg` edits the copy, so
  # checking and hashing the original would describe a file that is not the
  # one in the package. The build output in target/ is never touched.
  cp "$b" "$OUTDIR/" || { falha "$name: copy failed"; continue; }
  d="$OUTDIR/$name"
  tipo=$(file -b "$d")
  printf '%s' "$tipo" | grep -qE "$FORMATO" || falha "$name: unexpected format: $tipo"
  fixar_prefixo_ffmpeg "$d" "$name"
  # After the rewrite, not before: `install_name_tool` writing a longer path
  # can change the file's size, and a table that reports the size of a file
  # nobody ships is a table that lies.
  bytes=$(wc -c < "$d" | tr -d ' ')
  [ "$bytes" -gt 100000 ] || falha "$name: only $bytes bytes"
  conferir_dependencias "$d" "$name"
  sum=$(shasum -a 256 "$d" | cut -d' ' -f1)
  printf '%-24s %12s  %s\n' "$name" "$bytes" "$sum"
  printf '%s  %s\n' "$sum" "$name" >> "$OUTDIR/SHA256SUMS"
  ROWS="$ROWS| \`$name\` | $bytes | \`${sum:0:16}\` |
"
done <<< "$BINARIOS"

# The stamp proves this binary is from THIS commit. It is the same question
# the deploy asks /healthz at the end, asked before any machine receives it.
daemon="$OUTDIR/oxdesk"
[ "$TARGET" = windows ] && daemon="$OUTDIR/oxdesk.exe"
if [ -f "$daemon" ]; then
  grep -aqF "$OXDESK_BUILD" "$daemon" || falha "oxdesk does not carry the stamp $OXDESK_BUILD"
fi

if [ "$failures" -ne 0 ]; then
  echo "$failures problem(s): not packaged"
  exit 1
fi

tar -czf "$OUTDIR.tar.gz" -C dist "$(basename "$OUTDIR")"
pacote_bytes=$(wc -c < "$OUTDIR.tar.gz" | tr -d ' ')
echo "package: $OUTDIR.tar.gz ($pacote_bytes bytes)"

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### $TARGET: build #$OXDESK_BUILD_N \`$OXDESK_BUILD\`"
    echo
    echo "| binary | bytes | sha256 |"
    echo "|---|---:|---|"
    printf '%s' "$ROWS"
    echo
    echo "Package: \`$(basename "$OUTDIR").tar.gz\`, $pacote_bytes bytes."
  } >> "$GITHUB_STEP_SUMMARY"
fi
