#!/usr/bin/env bash
# Builds and tests the WHOLE repo, on every target we use.
#
# It exists because breaking what is not compiled is silent: renaming
# `battery_level` to `battery_status` left `hidpp/src/bin/probe.rs` broken
# for hours, because I had only been building `logid` and `agent`. A loud
# error at the right moment costs less than a quiet one found later.
#
# usage:  bash scripts/check.sh
set -uo pipefail
cd "$(dirname "$0")/.."

falhas=0
run() {
  local titulo="$1"; shift
  printf '  %-46s' "$titulo"
  if out=$("$@" 2>&1); then
    # A warning does not break the build, and so it goes unnoticed.
    # Showing it here is what gets a new warning noticed the day it shows
    # up.
    local avisos
    avisos=$(printf '%s\n' "$out" | grep -c '^warning:')
    if [ "$avisos" -gt 0 ]; then
      echo "ok ($avisos warning(s))"
      printf '%s\n' "$out" | grep '^warning:' | sed 's/^/      /' | head -8
    else
      echo "ok"
    fi
  else
    echo "FAILED"
    echo "$out" | grep -E '^(error|warning)' -A4 | head -20 | sed 's/^/      /'
    falhas=$((falhas + 1))
  fi
}

# `agent` and `winput` are Windows-only (pure Win32): on Linux `winput`
# compiles EMPTY because of `#![cfg(windows)]`, and the agent cannot find
# the functions. Not a regression, just no native target here.
echo "== native (linux) =="
for c in hidpp logid ota logistream; do
  run "$c: build" cargo build --release --manifest-path "$c/Cargo.toml"
done
# `logistream` tests here and not only in the cross: 85 of them exist and
# NONE of them ran in this suite -- including the pixel ones in `cursor.rs`,
# which were moved out of `capture_win.rs` precisely so they would run.
for c in hidpp logid ota logistream; do
  run "$c: test" cargo test --release --manifest-path "$c/Cargo.toml"
done

echo "== cross (windows) =="
# agent and winput only exist on Windows; logid runs on both.
#
# `logistream` is here for the reason in this file's header: half of it
# (`capture_win`, `input_win`, `host_win`, `cursor_win`) is compiled ONLY
# for Windows, so until this line existed those files were verified by
# whoever remembered to cross-build by hand before a deploy -- and the
# cursor overlay (specs/010) was written inside exactly that blind spot.
#
# It goes in WHOLE, binaries and all. The Linux-only probes declare that
# themselves (`capture-probe`, `v1-record` and `kvm` carry a stub `main`
# for other targets), so there is no hand-picked list of binaries here --
# a list is a thing that quietly stops covering the file you just added.
for c in hidpp logid agent winput ota logistream; do
  run "$c: build windows" cargo build --release --target x86_64-pc-windows-gnu \
      --manifest-path "$c/Cargo.toml"
done

echo
if [ "$falhas" -eq 0 ]; then
  echo "all green"
else
  echo "$falhas target(s) with problems"
fi
exit "$falhas"
