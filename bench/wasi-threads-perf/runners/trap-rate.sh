#!/bin/bash
# How often does 4-thread mergesort trap on our runtime? Ten runs, counted.
set -u
cd "$(dirname "$0")/.." || exit 1
root="$(cd ../.. && pwd)"
cli="$root/target/release/wnjr-cli"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
out="$1"; : > "$out"
pass=0; trap=0
for i in $(seq 1 10); do
  msg=$("$cli" "$wasm" mergesort 4000000 4 2>&1 >/dev/null)
  rc=$?
  if [ $rc -eq 0 ]; then
    pass=$((pass+1)); echo "run $i: ok" >> "$out"
  else
    trap=$((trap+1)); echo "run $i: exit=$rc ${msg}" >> "$out"
  fi
done
echo "pass=$pass trap=$trap" >> "$out"
echo "ALL DONE" >> "$out"
