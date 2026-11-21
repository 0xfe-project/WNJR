#!/bin/bash
# The compiler built by itself is a correct compiler, not only a fixed point.
#
#   test/bootstrap-emit.sh
#
# bootstrap.sh compares the module wcc emits with the module the compiler wcc
# built emits, and they are equal. That is a property of the chain and not of the
# compiler: a frontend that gives the same wrong answer at every stage is a
# perfect fixed point. This is the other half of the claim. Two compilers -- the
# host binary, and the one it built, running on WNJR -- are given the same
# sources with the same words, and their output has to be equal:
#
#   -E and --dump-ast  every case of chibicc's test set. Neither mode needs a
#                      libc, so a case costs a fraction of a second, and between
#                      them they cover the preprocessor, the parser and the type
#                      checker.
#   an emitted module  a few cases with the whole libc, compared byte for byte:
#                      code generation end to end. Deliberately few, because the
#                      guest spends about fourteen seconds on each, and
#                      bootstrap.sh already compares 274 files' worth.
#
# This is not decoration. A char argument to printf was stored one byte wide in
# an eight-byte slot, so "%d" of (char)-1 read whatever the slot had held before
# and answered 255 or 0x5b90ff depending on the run. Every stage of the bootstrap
# produced that same wrong code and the fixed point held; the first case of
# --dump-ast diverged here. The regression is test/libc-io/vararg.c.
#
# Needs the self-built compiler: run bootstrap.sh first, or this script does.

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
run="$root/target/debug/njvm-run"
work="$root/.bootstrap/emit"
tdir="$root/vendor/chibicc"
tests="$tdir/test"

cd "$root" || exit 1
. "$here/not-comparable.sh"
. "$here/libc-units.sh"

make -C wnjr-cc >/dev/null || { echo "FAIL: cannot build wcc"; exit 1; }
[ -x "$run" ] || cargo build --offline -q --bin njvm-run || { echo "FAIL: cannot build njvm-run"; exit 1; }
if [ ! -f .bootstrap/stage2.mp ]; then
  wnjr-cc/test/bootstrap.sh > /dev/null || { echo "FAIL: no self-built compiler"; exit 1; }
fi
mkdir -p "$work" || exit 1

# The guest's words: argv[0] is ./wnjr-cc/wcc, so the compiler finds the headers
# and the libc beside itself exactly as the host binary does, and then every word
# the host binary was given. Paths are relative to the top of the tree, because a
# guest sees the working directory as its root, and the host run uses the same
# relative paths so that __FILE__ agrees.
guest() { # $1 = file for stdout; the rest = the compiler's words
  local out=$1
  shift
  local words=(--guest-arg ./wnjr-cc/wcc)
  local w
  for w in "$@"; do words+=(--guest-arg "$w"); done
  "$run" .bootstrap/stage2.mp "${words[@]}" bootstrap_main > "$out" 2> "$out.err"
  local rc=$?
  # njvm-run prints the export's return value as the last line of stdout; the
  # compiler's own output is everything above it.
  if [ "$(tail -1 "$out")" = 0 ]; then
    sed -e '$d' "$out" > "$out.trim"
    mv "$out.trim" "$out"
  fi
  return $rc
}

cases=""
skipped=0
for src in "$tests"/*.c; do
  name=$(basename "$src" .c)
  if reason=$(not_comparable "$name.c"); then
    echo "[skip] $name: $reason"
    skipped=$((skipped + 1))
    continue
  fi
  cases="$cases $name"
done

pass=0
fail=0
failed=""

for mode in -E --dump-ast; do
  tag=${mode#--}
  tag=${tag#-}
  for name in $cases; do
    host="$work/host.$name.$tag"
    gst="$work/guest.$name.$tag"
    ./wnjr-cc/wcc "$mode" "vendor/chibicc/test/$name.c" > "$host" 2>&1
    if ! guest "$gst" "$mode" "vendor/chibicc/test/$name.c"; then
      echo "[FAIL] $name $mode: the self-built compiler exited nonzero"
      head -2 "$gst.err" | sed "s/^/        /"
      fail=$((fail + 1)); failed="$failed $name/$mode"; continue
    fi
    if cmp -s "$host" "$gst"; then
      pass=$((pass + 1))
    else
      echo "[FAIL] $name $mode: the two compilers disagree"
      diff "$host" "$gst" | head -6 | sed "s/^/        /"
      fail=$((fail + 1)); failed="$failed $name/$mode"
    fi
  done
done

# Code generation, with the real libc linked in. The libc units are given as
# relative paths so that both compilers see the same __FILE__ strings.
libc_units wnjr-cc-libc
for name in arith initializer control; do
  host="$work/$name.host.mp"
  gst=".bootstrap/emit/$name.guest.mp"
  rm -f "$gst"
  if ! ./wnjr-cc/wcc -o "$host" $units "vendor/chibicc/test/$name.c" vendor/chibicc/test/common; then
    echo "[FAIL] $name: the host compiler rejected the case"
    fail=$((fail + 1)); failed="$failed $name/module"; continue
  fi
  if ! guest "$work/$name.guest.log" -o "$gst" $units "vendor/chibicc/test/$name.c" vendor/chibicc/test/common; then
    echo "[FAIL] $name: the self-built compiler exited nonzero"
    fail=$((fail + 1)); failed="$failed $name/module"; continue
  fi
  if cmp -s "$host" "$gst"; then
    pass=$((pass + 1))
  else
    echo "[FAIL] $name: the emitted modules differ"
    fail=$((fail + 1)); failed="$failed $name/module"
  fi
done

echo
echo "bootstrap-emit: $pass identical, $fail differ ($skipped not comparable, listed above)"
[ -n "$failed" ] && echo "failed:$failed"
[ $fail -eq 0 ]
