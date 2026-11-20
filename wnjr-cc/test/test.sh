#!/bin/bash
# wnjr-cc preprocessor test: compare -E output byte-for-byte with vendor chibicc
set -e

WNJR_CC="$(cd "$(dirname "$0")/.." && pwd)/wnjr-cc"
CHIBICC_TESTSUITE="/Users/lemonhx/Desktop/tmp/Silverfir-nano/vendor/chibicc/test"

# Build vendor chibicc in a temp directory (chibicc builds in-place)
VENDOR_TMP="${VENDOR_TMP:-$(mktemp -d)}"
if [ ! -f "$VENDOR_TMP/chibicc" ]; then
  echo "Building vendor chibicc in $VENDOR_TMP..." >&2
  cp -r /Users/lemonhx/Desktop/tmp/Silverfir-nano/vendor/chibicc/* "$VENDOR_TMP/"
  (cd "$VENDOR_TMP" && make chibicc >/dev/null 2>&1)
fi
VENDOR_CC="$VENDOR_TMP/chibicc"

# Find all .c test files
TESTS=($CHIBICC_TESTSUITE/*.c)
PASS=0
FAIL=0
TOTAL=${#TESTS[@]}

for t in "${TESTS[@]}"; do
  base=$(basename "$t")
  
  # Run both preprocessors with the same flags chibicc tests use
  OUT_V=$(mktemp)
  OUT_W=$(mktemp)
  
  "$VENDOR_CC" -E -Iinclude -Itest "$t" > "$OUT_V" 2>/dev/null || true
  "$WNJR_CC" -E "$t" > "$OUT_W" 2>/dev/null || true
  
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
echo "Results: $PASS/$TOTAL passed, $FAIL/$TOTAL failed"
exit $FAIL
