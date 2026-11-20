#!/bin/bash
# wnjr-cc AST dump test: compare --dump-ast output byte-for-byte with a scratch
# copy of vendor chibicc that has test/vendor-dump.patch applied and the shared
# dump.c dropped in. vendor/chibicc itself is never modified.
#
# Usage: test/dump-test.sh [file.c ...]   (default: all vendor/chibicc/test/*.c
#        plus test/cases/*.c)
# A file passes only if BOTH compilers exit 0 and the dumps are identical and
# non-empty. Exit status is the number of failing files.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
WNJR_CC="$(cd "$HERE/.." && pwd)/wnjr-cc"
VENDOR_SRC="${VENDOR_SRC:-/Users/lemonhx/Desktop/tmp/Silverfir-nano/vendor/chibicc}"
CHIBICC_TESTSUITE="$VENDOR_SRC/test"

VENDOR_TMP="${VENDOR_TMP:-$(mktemp -d)}"
if [ ! -x "$VENDOR_TMP/chibicc" ]; then
  echo "Building patched scratch copy of vendor chibicc in $VENDOR_TMP..." >&2
  cp -R "$VENDOR_SRC"/. "$VENDOR_TMP/"
  cp "$HERE/../dump.c" "$VENDOR_TMP/dump.c"
  (cd "$VENDOR_TMP" && patch -s -p1 < "$HERE/vendor-dump.patch" && make chibicc >/dev/null 2>&1) || {
    echo "failed to build patched vendor chibicc" >&2; exit 99; }
fi
VENDOR_CC="$VENDOR_TMP/chibicc"

# Same flags and cwd for both compilers. cwd is the vendor root because
# pragma-once.c includes "test/pragma-once.c". sysinc/ holds stub system
# headers (macOS has no /usr/include); it is searched after -Itest.
# One set of headers for both compilers. Each would otherwise fall back to its
# own include directory, and wnjr-cc's <stdarg.h> now denotes va_list as a
# pointer into the save area rather than GCC's array of one — a deliberate
# divergence that would show up here as a different AST for varargs.c and say
# nothing about either parser.
FLAGS=(--dump-ast "-I$VENDOR_SRC/include" "-I$VENDOR_SRC" "-I$CHIBICC_TESTSUITE" "-I$HERE/sysinc")

if [ $# -gt 0 ]; then
  # absolute paths: the script cd's to the vendor root below
  TESTS=()
  for a in "$@"; do TESTS+=("$(cd "$(dirname "$a")" && pwd)/$(basename "$a")"); done
else
  shopt -s nullglob
  TESTS=("$CHIBICC_TESTSUITE"/*.c "$HERE"/cases/*.c)
fi

cd "$VENDOR_SRC"

PASS=0; FAIL=0; TOTAL=${#TESTS[@]}
for t in "${TESTS[@]}"; do
  base=$(basename "$t")
  OUT_V=$(mktemp); OUT_W=$(mktemp)
  "$VENDOR_CC" "${FLAGS[@]}" "$t" > "$OUT_V" 2>"$OUT_V.err"; RV=$?
  "$WNJR_CC"   "${FLAGS[@]}" "$t" > "$OUT_W" 2>"$OUT_W.err"; RW=$?
  LINES=$(wc -l < "$OUT_W" | tr -d ' ')
  if [ $RV -eq 0 ] && [ $RW -eq 0 ] && [ "$LINES" -gt 0 ] && cmp -s "$OUT_V" "$OUT_W"; then
    echo "[PASS] $base ($LINES lines)"
    PASS=$((PASS + 1))
  else
    echo "[FAIL] $base (vendor exit $RV, wnjr exit $RW, wnjr lines $LINES)"
    FAIL=$((FAIL + 1))
    head -3 "$OUT_V.err" "$OUT_W.err" 2>/dev/null
    diff -u "$OUT_V" "$OUT_W" | head -20
  fi
  rm -f "$OUT_V" "$OUT_W" "$OUT_V.err" "$OUT_W.err"
done

# Acceptance: the labels-as-values test must show both node kinds in its dump.
for t in "${TESTS[@]}"; do
  if [ "$(basename "$t")" = labelval.c ]; then
    D=$("$WNJR_CC" "${FLAGS[@]}" "$t")
    for k in LABEL_VAL GOTO_EXPR; do
      if ! grep -q " $k " <<<"$D"; then
        echo "[FAIL] labelval.c dump has no $k node"
        FAIL=$((FAIL + 1))
      fi
    done
  fi
done

echo
echo "Results: $PASS/$TOTAL passed, $FAIL/$TOTAL failed"
exit $FAIL
