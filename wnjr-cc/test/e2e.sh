#!/bin/bash
# End-to-end: C source -> NJVM-IR MessagePack (wnjr-cc) -> reader -> lowering ->
# WNJR, compared call by call against the host C compiler.
#
# There are no expected values in this repo: each case is compiled twice, once
# by wnjr-cc for WNJR and once by $CC for the host, and the two runs must agree.
#
#   test/e2e/<name>.c       the translation unit (no libc)
#   test/e2e/<name>.calls   one call per line: "<function> [int arg...]"
#
# Usage: test/e2e.sh [case...]

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
CC=${CC:-cc}
work=${TMPDIR:-/tmp}/wnjr-cc-e2e
mkdir -p "$work" || exit 1

if [ $# -gt 0 ]; then
  cases=""
  for c in "$@"; do cases="$cases $here/e2e/$c.c"; done
else
  cases=$(ls "$here"/e2e/*.c)
fi

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wnjr-cc"; exit 1; }
cargo build --offline -q --manifest-path "$root/Cargo.toml" -p wnjr-njvm-e2e --bin njvm-run \
  || { echo "FAIL: cannot build njvm-run"; exit 1; }
runner="$root/target/debug/njvm-run"
[ -x "$runner" ] || { echo "FAIL: $runner missing"; exit 1; }

pass=0
fail=0
failed_cases=""

for src in $cases; do
  name=$(basename "$src" .c)
  calls="$here/e2e/$name.calls"
  if [ ! -f "$calls" ]; then
    echo "[FAIL] $name: no $name.calls"
    fail=$((fail + 1))
    failed_cases="$failed_cases $name"
    continue
  fi

  msgpack="$work/$name.msgpack"
  if ! "$root/wnjr-cc/wnjr-cc" --emit-njvm -o "$msgpack" "$src"; then
    echo "[FAIL] $name: wnjr-cc rejected the source"
    fail=$((fail + 1))
    failed_cases="$failed_cases $name"
    continue
  fi

  # The host oracle: a driver generated from the same call table.
  decls="$work/$name-decls.c"
  body="$work/$name-body.c"
  labels="$work/$name-labels.txt"
  : > "$decls"
  : > "$body"
  : > "$labels"
  while IFS= read -r line; do
    case "$line" in ''|'#'*) continue ;; esac
    set -- $line
    fn=$1
    shift
    label="$fn"
    params=""
    call="$fn("
    first=1
    for a in "$@"; do
      label="$label $a"
      if [ -z "$params" ]; then params="int"; else params="$params, int"; fi
      if [ $first -eq 1 ]; then call="$call$a"; first=0; else call="$call, $a"; fi
    done
    call="$call)"
    [ -z "$params" ] && params="void"
    echo "int $fn($params);" >> "$decls"
    printf '%s\n' "  printf(\"$label -> %d\\n\", $call);" >> "$body"
    echo "$label" >> "$labels"
  done < "$calls"
  sort -u "$decls" -o "$decls"

  driver="$work/$name-driver.c"
  {
    echo '#include <stdio.h>'
    cat "$decls"
    echo 'int main(void) {'
    cat "$body"
    echo '  return 0;'
    echo '}'
  } > "$driver"

  if ! "$CC" -O1 -o "$work/$name-native" "$src" "$driver" 2> "$work/$name-cc.log"; then
    echo "[FAIL] $name: the host compiler rejected the source"
    sed 's/^/        /' "$work/$name-cc.log"
    fail=$((fail + 1))
    failed_cases="$failed_cases $name"
    continue
  fi
  "$work/$name-native" > "$work/$name-native.out"

  # One instance runs every call in file order, exactly as the C driver does,
  # so a call sees what an earlier call wrote.
  wnjr_out="$work/$name-wnjr.out"
  "$runner" "$msgpack" --batch "$calls" > "$wnjr_out" 2> "$work/$name-run.err"
  code=$?
  if [ $code -ne 0 ] && [ $code -ne 3 ]; then
    echo "[FAIL] $name: njvm-run failed"
    head -3 "$work/$name-run.err" | sed 's/^/        /'
    fail=$((fail + 1))
    failed_cases="$failed_cases $name"
    continue
  fi
  if diff -u "$work/$name-native.out" "$wnjr_out" > "$work/$name.diff"; then
    n=$(wc -l < "$labels" | tr -d ' ')
    echo "[PASS] $name: $n/$n calls agree with the host compiler"
    pass=$((pass + 1))
  else
    echo "[FAIL] $name: differs from the host compiler"
    sed 's/^/        /' "$work/$name.diff"
    fail=$((fail + 1))
    failed_cases="$failed_cases $name"
  fi
done

echo
if [ $fail -eq 0 ]; then
  echo "Results: $pass cases passed, 0 failed"
else
  echo "Results: $pass cases passed, $fail failed:$failed_cases"
  exit 1
fi
