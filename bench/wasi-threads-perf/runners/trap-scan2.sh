#!/bin/bash
# Is the trap about thread count or about resident size? Same array size, fewer
# threads; and a bigger array with only two threads.
set -u
cd "$(dirname "$0")/.." || exit 1
root="$(cd ../.. && pwd)"
cli="$root/target/release/wnjr-cli"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
out="$1"; : > "$out"
try() {
  local n="$1"; local t="$2"
  printf "n=%-8s threads=%s : " "$n" "$t" >> "$out"
  "$cli" "$wasm" mergesort "$n" "$t" >> "$out" 2>&1
  echo "  [exit=$?]" >> "$out"
}
try 3000000 4
try 3500000 4
try 4000000 3
try 8000000 2
try 4000000 4
echo "ALL DONE" >> "$out"
