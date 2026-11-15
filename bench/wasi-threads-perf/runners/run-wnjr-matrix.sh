#!/bin/bash
# Run every benchmark on our own runtime, both straight from the shell and
# driven by bun. Reps are sequential: never two benchmark processes at once,
# the numbers would be a measure of the scheduler.
# usage: run-wnjr-matrix.sh <out.log> [reps]
set -u
cd "$(dirname "$0")/.." || exit 1
root="$(cd ../.. && pwd)"
out="$1"; reps="${2:-3}"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
cli="$root/target/release/wnjr-cli"
: > "$out"
run() {
  local bench="$1"; local n="$2"; local i
  for i in $(seq 1 "$reps"); do
    echo "=== wnjr-cli $bench n=$n rep=$i" >> "$out"
    "$cli" "$wasm" "$bench" "$n" 4 >> "$out" 2>&1
    echo "--- exit=$?" >> "$out"
    echo "=== bun->wnjr-cli $bench n=$n rep=$i" >> "$out"
    bun runners/bun-wnjr.mjs "$cli" "$wasm" "$bench" "$n" 4 >> "$out" 2>&1
    echo "--- exit=$?" >> "$out"
  done
}
run mergesort 4000000
run matmul 768
run pi 40000000
echo "ALL DONE" >> "$out"
