#!/bin/bash
# The escape analysis: which local variables may live in a wasm local (a
# register) and which have to keep a frame slot. --dump-escapes prints the answer
# for every local of every function, so this compares that list against what the
# C says it should be.
#
# The cases are the whole point. A variable is only a register when nothing can
# reach its storage: no "&", a whole-machine-word type, and not an atomic. Every
# entry below is a way that fails, next to the ones that must not.
#
# Usage: test/escape.sh        Exit status is the number of failed expectations.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
CC="$(cd "$HERE/.." && pwd)/wcc"
ROOT="$(cd "$HERE/../.." && pwd)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/cases.c" <<'EOF'
int use(int *p);
int *cell;

struct box { int a; };

void shapes(void) {
  int stored = 1;
  int passed = 2;
  int deref = 3;
  int array_like[2];
  struct box b;
  char narrow = 0;
  int plain = 4;
  int incdec = 5;
  int opassign = 6;
  int *pointer = 0;

  cell = &stored;
  use(&passed);
  *&deref = 7;
  array_like[0] = 1;
  b.a = 1;
  narrow++;
  plain++;
  incdec--;
  opassign += 2;
  (void)pointer;
}
EOF

OUT="$TMP/cases.txt"
if ! "$CC" --dump-escapes "$TMP/cases.c" > "$OUT" 2> "$TMP/err"; then
  echo "[FAIL] --dump-escapes failed:"
  head -3 "$TMP/err"
  exit 1
fi

PASS=0
FAIL=0

# expect <variable> <frame|register> <why>
expect() {
  local name=$1 want=$2 why=$3 got
  # A name may be declared in more than one function of the unit; every
  # declaration of it has to agree, so the set of answers must be one word.
  got=$(awk -v n="$name" '$1 == n { print $2 }' "$OUT" | sort -u)
  if [ "$got" = "$want" ]; then
    echo "[PASS] $name is $want -- $why"
    PASS=$((PASS + 1))
  else
    echo "[FAIL] $name should be $want ($why), got ${got:-nothing}"
    FAIL=$((FAIL + 1))
  fi
}

expect stored    frame    "its address is stored in a global"
expect passed    frame    "its address is passed to a call"
expect deref     frame    "&x appears, even though it is dereferenced at once"
expect array_like frame   "an array is a place, not a value"
expect b         frame    "a struct is a place, not a value"
expect narrow    frame    "a char is stored through a byte store, which truncates"
# This one is the reverse of the others: chibicc keeps it for the x86 backend,
# where alloca records its stack adjustment in a variable. Nothing here reads it --
# the stack comes back from the frame pointer -- so it must NOT hold a frame slot,
# or every function would keep a frame for an object no one looks at.
expect __alloca_size__ register "nothing here reads it; the x86 backend used it"
expect plain     register "nothing takes its address"
expect incdec    register "++ does not take the address of a plain variable"
expect opassign  register "+= does not take the address of a plain variable"

# The benchmarks are the reason this analysis exists: their loop counters and
# accumulators are exactly the variables that must end up in registers.
for b in pi matmul mergesort nqueens; do
  BOUT="$TMP/$b.txt"
  if ! "$CC" --dump-escapes "$ROOT/njvm-cc/bench/$b.c" > "$BOUT" 2> "$TMP/err"; then
    echo "[FAIL] --dump-escapes failed on $b.c"
    head -3 "$TMP/err"
    FAIL=$((FAIL + 1))
    continue
  fi
  case $b in
    pi)       want="x y hits" ;;
    matmul)   want="i j k s sum" ;;
    mergesort) want="i h mid" ;;
    nqueens)  want="free bit" ;;
  esac
  for v in $want; do
    got=$(awk -v n="$v" '$1 == n { print $2 }' "$BOUT" | sort -u)
    if [ "$got" = register ]; then
      echo "[PASS] $b.c: $v is in a register"
      PASS=$((PASS + 1))
    else
      echo "[FAIL] $b.c: $v should be a register, got ${got:-not found}"
      FAIL=$((FAIL + 1))
    fi
  done
done

echo
echo "escape: $PASS passed, $FAIL failed"
exit $FAIL
