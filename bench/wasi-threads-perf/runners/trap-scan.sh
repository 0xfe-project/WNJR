#!/bin/bash
# Narrow the mergesort trap: vary the element count and the thread count.
set -u
cd "$(dirname "$0")/.." || exit 1
root="$(cd ../.. && pwd)"
cli="$root/target/release/wnjr-cli"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
out="$1"; : > "$out"
for n in 1000 100000 500000 1000000 2000000 4000000; do
  for t in 1 2 4; do
    printf "n=%-8s threads=%s : " "$n" "$t" >> "$out"
    "$cli" "$wasm" mergesort "$n" "$t" >> "$out" 2>&1
    echo "  [exit=$?]" >> "$out"
  done
done
echo "ALL DONE" >> "$out"
