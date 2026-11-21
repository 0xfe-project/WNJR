#!/bin/bash
# A big initializer must not cost stack proportional to the number of elements.
#
# Every part of an initializer becomes a link of a comma expression, and every
# pass over the tree walks a chain by recursion: add_type, gen_expr, the dump.
# One link per element therefore meant one stack frame per element, and
# "int t[50000] = {...}" — a local array with fifty thousand numbers, legal C —
# crashed the compiler on the host and tripped the stack guard on the guest. It
# was never the array that needed the memory; it was the shape of the tree.
#
# The tree is folded in pairs now, so its depth is the logarithm of the element
# count and the compiler's stack stops depending on the size of the program.
# The bound below makes that a test rather than a claim: 256 KB is four times
# what the compiler needs for anything in this repository, and a fifth of what
# it needed for five thousand elements before the fix.
#
# The initializer is generated here; a source file with twenty thousand numbers
# in it would be unreadable and would say nothing a loop does not.
#
# The program is compiled and not run. A local array of twenty thousand elements
# needs more wasm locals than one function may declare (the store for each
# element costs a temporary of its own, and wasm stops at fifty thousand), so the
# module would be refused at instantiation. That is a separate limit -- a local
# initializer of a few thousand elements is about where it starts -- and it is
# not what this test is about. What an initializer's values and their order look
# like is covered by test/e2e/initorder.c, which compares against the host
# compiler.

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cc="$root/wnjr-cc/wcc"
runner="$root/target/debug/njvm-run"
work=${TMPDIR:-/tmp}/wnjr-cc-init-large
rm -rf "$work"; mkdir -p "$work" || exit 1

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wcc"; exit 1; }
cargo build --offline -q --manifest-path "$root/Cargo.toml" -p wnjr-njvm-e2e --bin njvm-run \
  || { echo "FAIL: cannot build njvm-run"; exit 1; }

n=20000
awk -v n="$n" 'BEGIN {
  printf "int main(void) {\n  int t[%d] = {", n;
  for (i = 0; i < n; i++) printf "%s%d", (i ? "," : ""), i % 7;
  printf "};\n  return (int)t[%d];\n}\n", n - 1;
}' > "$work/big.c"

# The stack bound applies to the compiler alone: the emitted program has a frame
# of its own, and how much stack it uses is its business, not this test's.
( ulimit -s 256; "$cc" -o "$work/big.mp" "$work/big.c" ) 2> "$work/err"
if [ $? -ne 0 ]; then
  echo "[FAIL] the compiler did not survive a 256 KB stack on a 20000 element initializer"
  sed -n '1,5p' "$work/err"
  exit 1
fi

echo "[pass] a 20000 element initializer compiled under a 256 KB stack"
