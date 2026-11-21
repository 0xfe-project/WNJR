#!/bin/bash
# What a translation unit depends on: test/deps-test.sh
#
#   test/deps-test.sh
#
# A unit's dependency list is what a cache key has to contain and what the link
# step has to wait for, so it has to be exactly right: every file the
# preprocessor read or decided not to read, once each, and nothing that only
# looked reachable.
#
# The properties asserted below, one per case:
#   - the unit itself comes first, and a unit with no includes depends on
#     nothing else
#   - a header reached through a musl wrapper is listed under its resolved path:
#     src/include/stdio.h and include/stdio.h are two different files, each once
#   - a header behind "#if 0" or an undefined ifdef is not a dependency
#   - a header skipped because its own include guard was already defined still
#     is a dependency, because the guard is what decided the skip
#   - a user header beside the unit is a dependency too
#   - every line is an absolute path that exists, and no line repeats

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
cc="$root/wnjr-cc/wcc"

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wcc"; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

ok()  { pass=$((pass + 1)); echo "[PASS] $1"; }
bad() { fail=$((fail + 1)); echo "[FAIL] $1"; [ -n "${2:-}" ] && echo "       $2"; return 0; }

# Run --deps on a unit; the list lands in $work/out.
deps() { "$cc" --deps "$1" > "$work/out" 2> "$work/err" || { cat "$work/err" >&2; return 1; }; }

# The path the list should carry for a file: absolute, symlinks resolved.
real_of() { ( cd "$(dirname "$1")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$1")" ); }

# --- 1. a bare unit depends on nothing but itself -----------------------------

cat > "$work/bare.c" <<'EOF'
int main(void) { return 0; }
EOF

if deps "$work/bare.c"; then
  lines=$(wc -l < "$work/out" | tr -d ' ')
  if [ "$lines" = 1 ] && [ "$(cat "$work/out")" = "$(real_of "$work/bare.c")" ]; then
    ok "bare unit: the list is the unit itself, once"
  else
    bad "bare unit: the list is the unit itself, once" "$(cat "$work/out")"
  fi
else
  bad "bare unit: --deps failed"
fi

# --- 2. stdio.h, and only what stdio.h reaches --------------------------------

cat > "$work/stdio.c" <<'EOF'
#include <stdio.h>
int main(void) { printf("hi\n"); return 0; }
EOF

if deps "$work/stdio.c"; then
  head -1 "$work/out" | grep -qx "$(real_of "$work/stdio.c")" \
    && ok "unit first" || bad "unit first" "$(head -1 "$work/out")"

  # Two different files carry the name stdio.h: musl's public one, and the
  # wrapper in src/include whose whole body is "#include_next <stdio.h>". Both
  # are real dependencies; each must appear once, under one resolved path.
  n=$(grep -c 'musl/include/stdio\.h$' "$work/out")
  [ "$n" = 1 ] && ok "stdio.h once, not once per spelling" || bad "stdio.h once" "$n entries"
  n=$(grep -c 'musl/src/include/stdio\.h$' "$work/out")
  [ "$n" = 1 ] && ok "the src/include wrapper once" || bad "the src/include wrapper once" "$n entries"

  grep -q 'stdlib\.h$' "$work/out" \
    && bad "stdlib.h absent (stdio.h does not reach it)" "$(cat "$work/out")" \
    || ok "stdlib.h absent (stdio.h does not reach it)"
  grep -q 'math\.h$' "$work/out" \
    && bad "math.h absent" "$(cat "$work/out")" || ok "math.h absent"
else
  bad "stdio unit: --deps failed"
fi

# --- 3. a header that no taken branch reaches is not a dependency -------------

cat > "$work/dead.c" <<'EOF'
#if 0
#include <math.h>
#endif
#ifdef NOT_DEFINED
#include <stdlib.h>
#endif
int main(void) { return 0; }
EOF

if deps "$work/dead.c"; then
  grep -q 'math\.h$' "$work/out" \
    && bad "#if 0 header absent" "$(cat "$work/out")" || ok "#if 0 header absent"
  grep -q 'stdlib\.h$' "$work/out" \
    && bad "undefined-ifdef header absent" "$(cat "$work/out")" \
    || ok "undefined-ifdef header absent"
else
  bad "dead branch unit: --deps failed"
fi

# --- 4. a guarded header reached twice is still a dependency ------------------

mkdir -p "$work/diamond"
cat > "$work/diamond/b.h" <<'EOF'
#ifndef B_H
#define B_H
typedef int b_t;
#endif
EOF
cat > "$work/diamond/a.h" <<'EOF'
#include "b.h"
EOF
cat > "$work/diamond/c.h" <<'EOF'
#include "b.h"
EOF
cat > "$work/diamond/diamond.c" <<'EOF'
#include "a.h"
#include "c.h"
int main(void) { return 0; }
EOF

if deps "$work/diamond/diamond.c"; then
  n=$(grep -c 'diamond/b\.h$' "$work/out")
  [ "$n" = 1 ] && ok "guarded header: listed once" || bad "guarded header: listed once" "$n entries"
  if grep -q 'diamond/a\.h$' "$work/out" && grep -q 'diamond/c\.h$' "$work/out"; then
    ok "user headers are dependencies"
  else
    bad "user headers are dependencies" "$(cat "$work/out")"
  fi
else
  bad "diamond unit: --deps failed"
fi

# --- 5. every line is a path that exists, and no line repeats -----------------

for unit in "$work/bare.c" "$work/stdio.c" "$work/dead.c" "$work/diamond/diamond.c"; do
  [ -f "$unit" ] || continue
  name=$(basename "$unit" .c)
  deps "$unit" || { bad "$name: --deps failed"; continue; }

  dup=$(sort "$work/out" | uniq -d)
  [ -z "$dup" ] && ok "$name: no repeats" || bad "$name: no repeats" "$dup"

  missing=""
  while IFS= read -r p; do
    case "$p" in
      /*) [ -e "$p" ] || missing="$p (missing)";;
      *)  missing="$p (not absolute)";;
    esac
  done < "$work/out"
  [ -z "$missing" ] && ok "$name: every entry is an absolute path that exists" \
                    || bad "$name: every entry is an absolute path that exists" "$missing"
done

echo
echo "Results: $pass passed, $fail failed"
exit $fail
