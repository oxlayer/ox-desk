#!/usr/bin/env bash
# Compila e testa TODO o repo, em todos os alvos que usamos.
#
# Existe porque quebrar o que nao se compila e silencioso: a renomeacao de
# `battery_level` para `battery_status` deixou `hidpp/src/bin/probe.rs`
# quebrado por horas, porque eu vinha compilando so `logid` e `agent`. Erro
# barulhento no momento certo custa menos que erro quieto achado depois.
#
# uso:  bash scripts/check.sh
set -uo pipefail
cd "$(dirname "$0")/.."

falhas=0
run() {
  local titulo="$1"; shift
  printf '  %-46s' "$titulo"
  if out=$("$@" 2>&1); then
    # Aviso nao derruba o build, e por isso passa batido. Mostrar aqui e
    # o que faz um aviso novo ser notado no dia em que aparece.
    local avisos
    avisos=$(printf '%s\n' "$out" | grep -c '^warning:')
    if [ "$avisos" -gt 0 ]; then
      echo "ok ($avisos aviso(s))"
      printf '%s\n' "$out" | grep '^warning:' | sed 's/^/      /' | head -8
    else
      echo "ok"
    fi
  else
    echo "FALHOU"
    echo "$out" | grep -E '^(error|warning)' -A4 | head -20 | sed 's/^/      /'
    falhas=$((falhas + 1))
  fi
}

# O `agent` e o `winput` sao Windows-only (Win32 puro): no Linux o
# `winput` compila VAZIO por `#![cfg(windows)]`, e o agent nao acha as
# funcoes. Nao e regressao, e so nao ter alvo nativo aqui.
echo "== nativo (linux) =="
for c in hidpp logid ota; do
  run "$c: build" cargo build --release --manifest-path "$c/Cargo.toml"
done
for c in hidpp logid ota; do
  run "$c: test" cargo test --release --manifest-path "$c/Cargo.toml"
done

echo "== cruzado (windows) =="
# O agent e o winput so existem no Windows; o logid roda nos dois.
for c in hidpp logid agent winput ota; do
  run "$c: build windows" cargo build --release --target x86_64-pc-windows-gnu \
      --manifest-path "$c/Cargo.toml"
done

echo
if [ "$falhas" -eq 0 ]; then
  echo "tudo verde"
else
  echo "$falhas alvo(s) com problema"
fi
exit "$falhas"
