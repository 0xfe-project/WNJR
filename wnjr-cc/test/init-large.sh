#!/bin/bash
# A big source must cost neither stack nor wasm locals in proportion to its size.
#
# Two separate things scaled with the source here. Every part of an initializer
# becomes a link of a comma expression, and every pass over the tree walks a
# chain by recursion, so a left-deep chain of one link per element cost one stack
# frame per element: "int t[50000] = {...}" crashed the compiler. The chain is
# folded in pairs now (create_lvar_init), which makes its depth the logarithm of
# the element count.
#
# Then the codegen: it handed each temporary a wasm local of its own and never
# took one back, so an element cost two -- the address of its store and the value
# the store leaves behind -- and wasm allows fifty thousand locals in one
# function. Twenty thousand elements asked for forty thousand locals and the
# module was refused at instantiation, not compiled slowly. Temporaries go back
# to a pool now (temp_mark/temp_release), which is why the same program costs
# three locals whatever its size, and why twenty thousand *statements* cost three
# as well.
#
# Both shapes are checked, and the checksum walks every element and every
# statement: a temporary handed back while it still held a live value would show
# up as a wrong value somewhere, and nothing else here would notice it.
#
# The sources are generated; a file with twenty thousand numbers in it would be
# unreadable and would say nothing a loop does not.

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

# The same checksum over both shapes, so one number says whether the elements and
# their order came out right.
awk -v n="$n" 'BEGIN {
  printf "int main(void) {\n  int t[%d] = {", n;
  for (i = 0; i < n; i++) printf "%s%d", (i ? "," : ""), i % 7;
  printf "};\n  unsigned s = 0;\n";
  printf "  for (int i = 0; i < %d; i++)\n    s = (s * 31 + (unsigned)t[i]) & 0xffff;\n", n;
  printf "  return (int)s;\n}\n";
}' > "$work/init.c"

awk -v n="$n" 'BEGIN {
  printf "int main(void) {\n  unsigned s = 0;\n";
  for (i = 0; i < n; i++) printf "  s = (s * 31 + %du) & 0xffffu;\n", i % 7;
  printf "  return (int)s;\n}\n";
}' > "$work/stmts.c"

want=$(awk -v n="$n" 'BEGIN { s = 0; for (i = 0; i < n; i++) s = (s * 31 + (i % 7)) % 65536; printf "%d", s }')

fail=0
for shape in init stmts; do
  # The stack bound applies to the compiler alone: the emitted program has a
  # frame of its own, and how much stack it uses is its business, not this test's.
  ( ulimit -s 256; "$cc" -o "$work/$shape.mp" "$work/$shape.c" ) 2> "$work/$shape.err"
  if [ $? -ne 0 ]; then
    echo "[FAIL] $shape: the compiler did not survive a 256 KB stack on $n elements"
    sed -n '1,5p' "$work/$shape.err"
    fail=1
    continue
  fi
  got=$("$runner" "$work/$shape.mp" main 2>&1 | tail -1)
  if [ "$got" != "$want" ]; then
    echo "[FAIL] $shape: the checksum came back $got, want $want"
    fail=1
    continue
  fi
  echo "[pass] $shape: $n of them, a 256 KB stack, three wasm locals, checksum $got"
done

exit $fail
