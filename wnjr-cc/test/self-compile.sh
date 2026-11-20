#!/bin/bash
# Stage B of the bootstrap: wnjr-cc compiles its own translation units.
#
# Two modes, because they answer two different questions.
#
#   test/self-compile.sh <name>   one TU. A front-end question: did the parser
#                                 and the type checker accept this file? A
#                                 symbol left undefined is not this TU's fault —
#                                 it lives in another TU or in the libc — so the
#                                 run is reported as accepted when that is the
#                                 only complaint.
#   test/self-compile.sh          every TU, one line each.
#   test/self-compile.sh --link   every TU plus the whole libc in one module.
#                                 A link question: what is still missing.
#
# The list of undefined symbols the link mode prints is the work left before
# wnjr-cc can compile itself.

set -u

mode=tus
case "${1:-}" in
  --link) mode=link ;;
  -*) echo "usage: self-compile.sh [--link] [tu...]"; exit 2 ;;
  *) [ $# -gt 0 ] && mode=one ;;
esac

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
libc="$root/wnjr-cc-libc"
cc="$root/wnjr-cc/wnjr-cc"
work=${TMPDIR:-/tmp}/wnjr-cc-self
mkdir -p "$work" || exit 1

. "$here/libc-units.sh"
libc_incs "$libc"
# wnjr-cc's own directory: the TUs include "chibicc.h".
incs="$incs -I$root/wnjr-cc"

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wnjr-cc"; exit 1; }

tus=$(echo "$root"/wnjr-cc/*.c)

if [ "$mode" = link ]; then
  libc_units "$libc"
  if ./wnjr-cc/wnjr-cc --emit-njvm $incs -DHAVE_MMAP=0 -o /dev/null $units $tus > "$work/link.log" 2>&1; then
    echo "link: ok"
    exit 0
  fi
  # One line per distinct complaint, with the place that asked for it.
  sed -e "s|$libc/||g" -e "s|$root/||g" "$work/link.log" | sort -u | head -60
  echo
  echo "link: $(sed -e "s|$libc/||g" -e "s|$root/||g" "$work/link.log" | sort -u | wc -l | tr -d ' ') distinct complaints"
  exit 1
fi

cases=""
if [ "$mode" = one ]; then
  for c in "$@"; do cases="$cases $c"; done
else
  for src in $tus; do cases="$cases $(basename "$src" .c)"; done
fi

ok=0; bad=0; failed=""
for name in $cases; do
  src="$root/wnjr-cc/$name.c"
  if [ ! -f "$src" ]; then echo "[FAIL] $name: no such TU"; bad=$((bad+1)); failed="$failed $name"; continue; fi
  if ./wnjr-cc/wnjr-cc --emit-njvm $incs -DHAVE_MMAP=0 -o /dev/null "$src" > "$work/$name.log" 2>&1; then
    ok=$((ok+1)); echo "[ok]   $name"
  elif ! grep -Ev "^(wnjr-cc: undefined: |wnjr-cc: [0-9]+ symbol)" "$work/$name.log" | grep -q .; then
    # Only undefined symbols: the front end took the file, the link has not run.
    ok=$((ok+1)); echo "[ok]   $name (front end; undefined left to the link)"
  else
    bad=$((bad+1)); failed="$failed $name"
    if [ "$mode" = one ]; then
      sed -e "s|$libc/||g" -e "s|$root/||g" "$work/$name.log" | head -30 | sed "s/^/       /"
    else
      echo "[fail] $name: $(sed -e "s|$libc/||g" -e "s|$root/||g" -e "s/ *\^ error//" "$work/$name.log" | head -1)"
    fi
  fi
done

echo
echo "self-compile: $ok ok, $bad failed"
[ -n "$failed" ] && echo "failed:$failed"
[ $bad -eq 0 ]
