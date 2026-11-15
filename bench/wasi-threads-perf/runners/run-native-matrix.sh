#!/bin/bash
# Rust native baseline: the same source, the same 4 threads, no wasm.
# usage: run-native-matrix.sh <out.log> [reps]
set -u
cd "$(dirname "$0")/.." || exit 1
out="$1"; reps="${2:-3}"
bin=target/release/wt-perf
: > "$out"
run() {
  local bench="$1"; local n="$2"; local i
  for i in $(seq 1 "$reps"); do
    echo "=== native $bench n=$n rep=$i" >> "$out"
    "$bin" "$bench" "$n" 4 >> "$out" 2>&1
    echo "--- exit=$?" >> "$out"
  done
}
run mergesort 4000000
run matmul 768
run pi 40000000
echo "ALL DONE" >> "$out"
