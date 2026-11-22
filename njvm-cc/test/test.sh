#!/bin/bash
# njvm-cc preprocessor test: compare -E output byte-for-byte with vendor chibicc
set -e

root=$(cd "$(dirname "$0")/../.." && pwd)
WNJR_CC="$root/njvm-cc"
CHIBICC_TESTSUITE="$root/vendor/chibicc/test"

# Build vendor chibicc in a temp directory (chibicc builds in-place)
VENDOR_TMP="${VENDOR_TMP:-$(mktemp -d)}"
if [ ! -f "$VENDOR_TMP/chibicc" ]; then
  echo "Building vendor chibicc in $VENDOR_TMP..." >&2
  cp -r $root/vendor/chibicc/* "$VENDOR_TMP/"
  (cd "$VENDOR_TMP" && make chibicc >/dev/null 2>&1)
fi
VENDOR_CC="$VENDOR_TMP/chibicc"

# Cases whose output cannot be compared this way are named with the reason and
# counted apart, never silently dropped: see not-comparable.sh.
. "$(cd "$(dirname "$0")" && pwd)/not-comparable.sh"

# Find all .c test files
TESTS=($CHIBICC_TESTSUITE/*.c)
PASS=0
FAIL=0
SKIP=0
TOTAL=${#TESTS[@]}

for t in "${TESTS[@]}"; do
  base=$(basename "$t")
  if reason=$(not_comparable "$base"); then
    echo "[skip] $base: $reason"
    SKIP=$((SKIP + 1))
    continue
  fi
  
  # Run both preprocessors with the same flags chibicc tests use
  OUT_V=$(mktemp)
  OUT_W=$(mktemp)
  
  "$VENDOR_CC" -E -Iinclude -Itest "$t" > "$OUT_V" 2>/dev/null || true
  "$WNJR_CC/wcc" -E "$t" > "$OUT_W" 2>/dev/null || true
  
  if cmp -s "$OUT_V" "$OUT_W"; then
    echo "[PASS] $base"
    PASS=$((PASS + 1))
  else
    echo "[FAIL] $base"
    FAIL=$((FAIL + 1))
    # Show first diff line for debugging
    diff -u "$OUT_V" "$OUT_W" | head -20 || true
  fi
  
  rm -f "$OUT_V" "$OUT_W"
done

echo
comparable=$((TOTAL - SKIP))
echo "Results: $PASS/$comparable passed, $FAIL/$comparable failed ($SKIP not comparable, listed above)"
exit $FAIL
