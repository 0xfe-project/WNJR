#!/bin/bash
# Run every benchmark under bun, driving the same Node runner.
# usage: run-bun-matrix.sh <out.log> [reps]
set -u
cd "$(dirname "$0")/.." || exit 1
out="$1"; reps="${2:-3}"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
: > "$out"
run() {
  local bench="$1"; local n="$2"; local i
  for i in $(seq 1 "$reps"); do
    echo "=== bun $bench n=$n rep=$i" >> "$out"
    bun runners/node-wasi-threads.mjs "$wasm" "$bench" "$n" 4 >> "$out" 2>&1
    echo "--- exit=$?" >> "$out"
  done
}
run mergesort 4000000
run matmul 768
run pi 40000000
echo "ALL DONE" >> "$out"
