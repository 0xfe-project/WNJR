#!/bin/bash
# chibicc's own test suite, run through wnjr-cc and wnjr-cc-libc on WNJR.
#
# Every case compiles test/common, the whole trimmed libc and test/<name>.c into
# one module, runs main on WNJR through njvm-run --batch, and compares stdout
# and exit status against the same two C files built by the host compiler.
# assert() prints "<expr> => <value>" for every assertion that holds and exits 1
# on the first that does not, so agreeing stdout means every value agreed: a
# test that only returned 0 would not say that.
#
#   test/chibicc-test.sh            every case, then a summary
#   test/chibicc-test.sh -v         every case, pass or fail
#   test/chibicc-test.sh <case>...  the named cases only
#
# A case this frontend cannot compile yet is listed with the reason and counted
# apart: a case that cannot be built must never quietly leave the denominator.

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
libc="$root/wnjr-cc-libc"
cc="$root/wnjr-cc/wnjr-cc"
tdir="$root/vendor/chibicc/test"
runner="$root/target/debug/njvm-run"
CC=${CC:-cc}
work=${TMPDIR:-/tmp}/wnjr-cc-chibicc
mkdir -p "$work" || exit 1

verbose=0
cases=""
for a in "$@"; do
  case "$a" in
    -v) verbose=1 ;;
    *) cases="$cases $a" ;;
  esac
done
if [ -z "$cases" ]; then
  for src in "$tdir"/*.c; do cases="$cases $(basename "$src" .c)"; done
fi

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wnjr-cc"; exit 1; }
cargo build --offline -q --manifest-path "$root/Cargo.toml" -p wnjr-njvm-e2e --bin njvm-run \
  || { echo "FAIL: cannot build njvm-run"; exit 1; }
[ -x "$runner" ] || { echo "FAIL: $runner missing"; exit 1; }

# The parts of wnjr-cc-libc that are not part of one program, and why.
skip_unit="libc-top-half/musl/src/env/__init_tls.c
libc-top-half/musl/src/thread/wasi-threads/pthread_create.c
dlmalloc/src/dlmalloc.c
libc-bottom-half/crt/crt1-command.c
libc-bottom-half/sources/__main_void.c"

units=""
while IFS= read -r f; do
  p=${f#./}
  for s in $skip_unit; do [ "$p" = "$s" ] && continue 2; done
  units="$units $libc/$p"
done < <(cd "$libc" && find . -name "*.c" | LC_ALL=C sort)

incs=""
for d in libc-top-half/musl/src/include libc-top-half/musl/src/internal \
         libc-top-half/musl/arch/wasm32 libc-top-half/musl/arch/generic \
         libc-top-half/musl/include libc-top-half/headers/private \
         libc-bottom-half/headers/public libc-bottom-half/headers/private \
         libc-bottom-half/cloudlibc/src libc-bottom-half/cloudlibc/src/include \
         generated/include dlmalloc/include include; do
  incs="$incs -I$libc/$d"
done

# A case this frontend cannot take, and why. A skipped case is printed on every
# run and reported apart, never as a pass.
skip_reason() {
  case "$1" in
    asm)
      echo "inline asm: this frontend has no asm statement, and wasm has no asm";;
    tls)
      echo "threads: this libc is single-threaded (the data segment is the TLS block and is never copied), so pthread_create is not in the tree; the case needs two threads to observe a thread-local at all";;
    atomic)
      echo "threads: three pthreads hammering one atomic with pthread_create/pthread_join, which this single-threaded libc does not have; the atomics themselves are covered by test/e2e/atomics.c";;
    *) return 1;;
  esac
}

pass=0; fail=0; skipped=0; failed=""

for name in $cases; do
  src="$tdir/$name.c"
  if [ ! -f "$src" ]; then
    echo "[FAIL] $name: no such case"; fail=$((fail+1)); failed="$failed $name"; continue
  fi
  if reason=$(skip_reason "$name"); then
    skipped=$((skipped+1)); echo "[skip] $name: $reason"; continue
  fi
  out="$work/$name"
  # -I"$root/vendor/chibicc": a case includes "test/..." by a path relative to the
  # tree it sits in, which is how chibicc own driver finds it.
  # Compiled from the chibicc tree with a relative path, because a case asserts its
  # own __FILE__ ("test/macro.c"), and chibicc own driver runs it that way. The
  # libc units are absolute and unaffected.
  if ! (cd "$root/vendor/chibicc" && "$cc" --emit-njvm -I"$tdir" $incs -DHAVE_MMAP=0 \
         -o "$out.mp" $units "$tdir/common" "test/$name.c") > "$out.compile" 2>&1; then
    echo "[FAIL] $name: wnjr-cc rejected the source"
    sed -e "s|$libc/||g" -e "s|$root/||g" "$out.compile" | head -3 | sed "s/^/        /"
    fail=$((fail+1)); failed="$failed $name"; continue
  fi
  echo main > "$out.calls"
  "$runner" "$out.mp" --batch "$out.calls" > "$out.out" 2> "$out.err"; wnjr_rc=$?
  # The batch driver prints one "<export> -> <value>" line per call, and the last
  # one is the return value of main, which the C driver never prints. A case that
  # exits before returning from main has no such line, and dropping the last line
  # anyway would throw away the message of the assertion that failed.
  if tail -1 "$out.out" | grep -q "^main -> "; then
    sed -e "$ d" "$out.out" > "$out.guest"
  else
    cp "$out.out" "$out.guest"
  fi
  # -xc: test/common has no .c suffix. alloca has no prototype in the case and
  # chibicc treats it as a builtin, so the host build is told what it is.
  $CC -w -xc -Dalloca=__builtin_alloca -I"$tdir" -o "$out.exe" "$src" "$tdir/common" \
    > "$out.hostcompile" 2>&1
  if [ $? -ne 0 ]; then
    # No host oracle: the run must still have passed every assertion.
    if [ $wnjr_rc -eq 0 ] && ! grep -q "expected but got" "$out.guest"; then
      pass=$((pass+1))
      [ $verbose -eq 1 ] && echo "[ok]   $name (exit status only: the host cc cannot build it)"
    else
      echo "[FAIL] $name: no host build and the run did not pass every assertion"
      fail=$((fail+1)); failed="$failed $name"
    fi
    continue
  fi
  "$out.exe" > "$out.host" 2>/dev/null; host_rc=$?
  # Some cases assert what this compiler does rather than what C requires: the
  # address order of two locals, the size clang gives an alloca. When the host
  # build fails one of its own assertions it is not an oracle for that case, and
  # the check becomes the one chibicc itself makes -- that every assertion in the
  # case holds.
  if [ $host_rc -ne 0 ] || grep -q "expected but got" "$out.host"; then
    if [ $wnjr_rc -eq 0 ] && ! grep -q "expected but got" "$out.guest"; then
      pass=$((pass+1))
      [ $verbose -eq 1 ] && echo "[ok]   $name (the host build fails this case; every assertion here holds)"
    else
      echo "[FAIL] $name: an assertion does not hold (exit $wnjr_rc)"
      grep "expected but got" "$out.guest" | head -4 | sed "s/^/        /"
      head -2 "$out.err" | sed "s/^/        /"
      fail=$((fail+1)); failed="$failed $name"
    fi
    continue
  fi
  if [ $wnjr_rc -eq $host_rc ] && cmp -s "$out.guest" "$out.host"; then
    pass=$((pass+1)); [ $verbose -eq 1 ] && echo "[ok]   $name"
  else
    echo "[FAIL] $name: exit $wnjr_rc vs host $host_rc"
    diff "$out.host" "$out.guest" | head -6 | sed "s/^/        /"
    head -2 "$out.err" | sed "s/^/        /"
    fail=$((fail+1)); failed="$failed $name"
  fi
done

echo
echo "chibicc: $pass passed, $fail failed, $skipped skipped"
[ -n "$failed" ] && echo "failed:$failed"
[ $fail -eq 0 ]
