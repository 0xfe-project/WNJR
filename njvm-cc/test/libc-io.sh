#!/bin/bash
# The libc's own probes: a program that reaches the host, so a passing run says
# the layer works and not only that it compiles.
#
# Each probe is test/libc-io/<name>.c. It is built against the whole libc, run
# on WNJR through njvm-cli run, and its stdout is compared against
# test/libc-io/<name>.expect — written out of the standard, not out of a run of
# the code under test, so the comparison cannot agree with a wrong answer.
#
# The probe runs in a scratch directory that the harness fills first, and the
# runner preopens that directory, so stat and open see real files with sizes
# this script chose.
#
#   test/libc-io.sh            every probe
#   test/libc-io.sh <name>...  the named probes

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
libc="$root/njvm-cc-libc"
cc="$root/njvm-cc/wcc"
runner="$root/target/debug/njvm-cli"
work=${TMPDIR:-/tmp}/njvm-cc-libc-io
rm -rf "$work"; mkdir -p "$work" || exit 1

. "$here/libc-units.sh"
libc_units "$libc"

make -C "$root/njvm-cc" >/dev/null || { echo "FAIL: cannot build njvm-cc"; exit 1; }
cargo build --offline -q --manifest-path "$root/Cargo.toml" -p njvm-cli --bin njvm-cli \
  || { echo "FAIL: cannot build njvm-cli"; exit 1; }

# What the probes look at. 1234 bytes is a size no rounding would produce, so a
# structure that came back zeroed or half-filled cannot pass for it.
python3 - "$work/probe-data.txt" <<'EOF'
import sys
open(sys.argv[1], 'wb').write(b'x' * 1234)
EOF
mkdir -p "$work/probe-dir"

cases=""
if [ $# -gt 0 ]; then
  for c in "$@"; do cases="$cases $c"; done
else
  for src in "$here"/libc-io/*.c; do cases="$cases $(basename "$src" .c)"; done
fi

pass=0; fail=0; failed=""
for name in $cases; do
  src="$here/libc-io/$name.c"
  want="$here/libc-io/$name.expect"
  if [ ! -f "$src" ]; then echo "[FAIL] $name: no such probe"; fail=$((fail+1)); failed="$failed $name"; continue; fi
  if [ ! -f "$want" ]; then echo "[FAIL] $name: no $name.expect"; fail=$((fail+1)); failed="$failed $name"; continue; fi
  if ! "$cc" -o "$work/$name.mp" $units "$src" > "$work/$name.compile" 2>&1; then
    echo "[FAIL] $name: njvm-cc rejected the probe"
    sed -e "s|$libc/||g" -e "s|$root/||g" "$work/$name.compile" | head -3 | sed "s/^/        /"
    fail=$((fail+1)); failed="$failed $name"; continue
  fi
  echo main > "$work/$name.calls"
  # The probe runs with the scratch directory as the working directory, and the
  # export's return value is main's, which the file already checks against zero.
  ( cd "$work" && "$runner" run "$work/$name.mp" --batch "$work/$name.calls" ) \
    > "$work/$name.out" 2> "$work/$name.err"
  rc=$?
  # njvm-cli run appends "main -> <value>"; the probe's own output is what is compared.
  if tail -1 "$work/$name.out" | grep -q "^main -> "; then
    sed -e "$ d" "$work/$name.out" > "$work/$name.guest"
  else
    cp "$work/$name.out" "$work/$name.guest"
  fi
  if [ $rc -ne 0 ]; then
    echo "[FAIL] $name: exit $rc"
    head -3 "$work/$name.err" | sed "s/^/        /"
    fail=$((fail+1)); failed="$failed $name"; continue
  fi
  if diff -u "$want" "$work/$name.guest" > "$work/$name.diff"; then
    pass=$((pass+1)); echo "[pass] $name"
  else
    echo "[FAIL] $name: output differs from $name.expect"
    sed "s/^/        /" "$work/$name.diff" | head -20
    fail=$((fail+1)); failed="$failed $name"
  fi
done

echo
echo "libc-io: $pass passed, $fail failed"
[ -n "$failed" ] && echo "failed:$failed"
[ $fail -eq 0 ]
