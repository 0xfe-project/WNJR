#!/bin/bash
# Single-thread column: quicksort plus the other three at threads=1.
# Nothing here spawns a thread, so whatever gap shows up is codegen and memory
# access, not thread machinery.
# usage: run-1t-matrix.sh <native|node|bun|wnjr|bunwnjr> <out.log> [reps]
set -u
cd "$(dirname "$0")/.." || exit 1
root="$(cd ../.. && pwd)"
host="${1:?host}"; out="${2:?out}"; reps="${3:-3}"
wasm=target/wasm32-wasip1-threads/release/wt-perf.wasm
cli="$root/target/release/wnjr-cli"
: > "$out"
one() {
  case "$host" in
    native)  ./target/release/wt-perf "$@" ;;
    node)    node runners/node-wasi-threads.mjs "$wasm" "$@" ;;
    bun)     bun runners/node-wasi-threads.mjs "$wasm" "$@" ;;
    wnjr)    "$cli" "$wasm" "$@" ;;
    bunwnjr) bun runners/bun-wnjr.mjs "$cli" "$wasm" "$@" ;;
    *) echo "unknown host $host" >&2; exit 2 ;;
  esac
}
run() {
  local bench="$1"; local n="$2"; local i
  for i in $(seq 1 "$reps"); do
    echo "=== $host $bench n=$n rep=$i" >> "$out"
    one "$bench" "$n" 1 >> "$out" 2>&1
    echo "--- exit=$?" >> "$out"
  done
}
run quicksort 4000000
run mergesort 4000000
run matmul 768
run pi 40000000
echo "ALL DONE" >> "$out"
