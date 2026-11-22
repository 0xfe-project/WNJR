#!/bin/bash
# Stage C: the compiler njvm-cc builds from itself reproduces itself on WNJR.
#
#   test/bootstrap.sh
#
# stage1  wcc, the host binary, compiles the compiler's own translation units
#         plus the whole libc into one NJVM module. That module is a compiler
#         that runs on WNJR.
# stage2  stage1, running on WNJR, compiles the same sources again.
# stage3  stage2, running on WNJR, compiles them a third time.
#
# Two byte comparisons, and both have to hold:
#
#   stage1 == stage2  the host's compiler and the compiler it built agree on
#                     every byte of the program they emit
#   stage2 == stage3  a fixed point: the compiler now rebuilds itself
#                     identically, so nothing drifts from one generation to the
#                     next
#
# Every stage after the first really executes on WNJR. Nothing here is compared
# on the host except the two files.
#
# Paths are relative and the run happens at the top of the tree: a guest sees the
# working directory as its root (see wasi_imports in njvm-e2e), so an
# absolute host path is not reachable from inside.

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
run="$root/target/debug/njvm-cli"
work="$root/.bootstrap"

cd "$root" || exit 1
. njvm-cc/test/libc-units.sh

make -C njvm-cc >/dev/null || { echo "FAIL: cannot build wcc"; exit 1; }
[ -x "$run" ] || cargo build --offline -q -p njvm-cli --bin njvm-cli || { echo "FAIL: cannot build njvm-cli"; exit 1; }

# The compiler's own sources plus the libc it links against: one program.
libc_units njvm-cc-libc
srcs="$units $(echo njvm-cc/*.c) njvm-cc/test/self-entry.c"

rm -rf "$work"
mkdir -p "$work" || exit 1

# The compiler's argv: it is invoked as ./njvm-cc/wcc so that it finds its libc
# beside itself, exactly as a shell would invoke it.
compile_on_wnjr() { # $1 module to run, $2 module to write, both as the guest sees them
  local args=(--guest-arg ./njvm-cc/wcc --guest-arg -o --guest-arg "$2")
  for a in $srcs; do args+=(--guest-arg "$a"); done
  # stdout is njvm-cli run's own "<value>" line; the module writes a file.
  "$run" run "$1" "${args[@]}" bootstrap_main > /dev/null
}

echo "sources: $(echo $srcs | wc -w | tr -d ' ') files"

echo "stage1: wcc builds the compiler"
if ! ./njvm-cc/wcc -o "$work/stage1.mp" $srcs 2> "$work/stage1.log"; then
  sed "s|$root/||g" "$work/stage1.log" | head -20
  echo "FAIL: the host compiler cannot build it"
  exit 1
fi
echo "       $(wc -c < "$work/stage1.mp" | tr -d ' ') bytes"

echo "stage2: that module, on WNJR, builds it again"
if ! compile_on_wnjr .bootstrap/stage1.mp .bootstrap/stage2.mp 2> "$work/stage2.log"; then
  head -20 "$work/stage2.log"
  echo "FAIL: the module built by wcc did not run on WNJR"
  exit 1
fi
echo "       $(wc -c < "$work/stage2.mp" | tr -d ' ') bytes"

echo "stage3: stage2, on WNJR, builds it a third time"
if ! compile_on_wnjr .bootstrap/stage2.mp .bootstrap/stage3.mp 2> "$work/stage3.log"; then
  head -20 "$work/stage3.log"
  echo "FAIL: stage2 did not run on WNJR"
  exit 1
fi
echo "       $(wc -c < "$work/stage3.mp" | tr -d ' ') bytes"

fail=0
if cmp -s "$work/stage1.mp" "$work/stage2.mp"; then
  echo "stage1 == stage2: the compiler built by itself emits the same bytes"
else
  echo "FAIL: stage1 and stage2 differ ($(cmp "$work/stage1.mp" "$work/stage2.mp" 2>&1))"
  fail=1
fi
if cmp -s "$work/stage2.mp" "$work/stage3.mp"; then
  echo "stage2 == stage3: fixed point"
else
  echo "FAIL: stage2 and stage3 differ ($(cmp "$work/stage2.mp" "$work/stage3.mp" 2>&1))"
  fail=1
fi
[ $fail -eq 0 ] && echo "bootstrap: ok"
exit $fail
