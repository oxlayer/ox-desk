#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 oxlayer
# SPDX-License-Identifier: AGPL-3.0-or-later

# Builds and tests the WHOLE repo, on every target we use.
#
# It exists because breaking what is not compiled is silent: renaming
# `battery_level` to `battery_status` left `hidpp/src/bin/probe.rs` broken
# for hours, because I had only been building `oxdesk` and `agent`. A loud
# error at the right moment costs less than a quiet one found later.
#
# usage:  bash scripts/check.sh
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

failures=0
run() {
  local title="$1"; shift
  printf '  %-46s' "$title"
  if out=$("$@" 2>&1); then
    # A warning does not break the build, and so it goes unnoticed.
    # Showing it here is what gets a new warning noticed the day it shows
    # up.
    local warnings
    warnings=$(printf '%s\n' "$out" | grep -c '^warning:')
    if [ "$warnings" -gt 0 ]; then
      echo "ok ($warnings warning(s))"
      printf '%s\n' "$out" | grep '^warning:' | sed 's/^/      /' | head -8
    else
      echo "ok"
    fi
  else
    echo "FAILED"
    echo "$out" | grep -E '^(error|warning)' -A4 | head -20 | sed 's/^/      /'
    failures=$((failures + 1))
  fi
}

# Instant, and first: a file published without a licence header is a
# licensing problem rather than a build problem, and it is the kind that
# gets noticed by whoever audits the repository instead of by whoever
# added the file. See specs/021.
echo "== licence =="
run "spdx: every source file declares one" bash scripts/spdx.sh
# The other half of specs/021: the gate that keeps the inbound grant real.
# What runs here is the gate's own behaviour, not this repository's state --
# it needs a pull request to have an opinion about, and a local run has
# none. What CAN rot locally is the decision table, so it ships a
# self-test of both directions and that is what this line exercises.
run "cla: the inbound-terms gate still gates" bash scripts/cla.sh --self-test
# Every string a person reads in the panel and the wizard has a Brazilian
# Portuguese translation (specs/022). Instant, and the failure names the
# string: a missing entry shows up as English on a Portuguese screen.
run "i18n: every visible string has pt-BR" python3 scripts/i18n-check.py

# `agent` and `winput` are Windows-only (pure Win32): on Linux `winput`
# compiles EMPTY because of `#![cfg(windows)]`, and the agent cannot find
# the functions. Not a regression, just no native target here.
echo "== native (linux) =="
for c in hidpp oxdesk ota stream; do
  run "$c: build" cargo build --release --manifest-path "$c/Cargo.toml"
done
# `oxdesk-stream` tests here and not only in the cross: 85 of them exist and
# NONE of them ran in this suite -- including the pixel ones in `cursor.rs`,
# which were moved out of `capture_win.rs` precisely so they would run.
for c in hidpp oxdesk ota stream; do
  run "$c: test" cargo test --release --manifest-path "$c/Cargo.toml"
done

echo "== cross (windows) =="
# agent and winput only exist on Windows; oxdesk runs on both.
#
# `stream` is here for the reason in this file's header: half of it
# (`capture_win`, `input_win`, `host_win`, `cursor_win`) is compiled ONLY
# for Windows, so until this line existed those files were verified by
# whoever remembered to cross-build by hand before a deploy -- and the
# cursor overlay (specs/010) was written inside exactly that blind spot.
#
# It goes in WHOLE, binaries and all. The Linux-only probes declare that
# themselves (`capture-probe`, `v1-record` and `kvm` carry a stub `main`
# for other targets), so there is no hand-picked list of binaries here --
# a list is a thing that quietly stops covering the file you just added.
for c in hidpp oxdesk agent winput ota stream; do
  run "$c: build windows" cargo build --release --target x86_64-pc-windows-gnu \
      --manifest-path "$c/Cargo.toml"
done

echo
if [ "$failures" -eq 0 ]; then
  echo "all green"
else
  echo "$failures target(s) with problems"
fi
exit "$failures"
