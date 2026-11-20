#!/bin/bash
# How much of wnjr-cc-libc (the trimmed wasi-libc) can wnjr-cc get through?
#
#   test/libc-parse.sh            count, then one line per file that fails
#   test/libc-parse.sh -v         every file, pass or fail
#
# Preprocesses and parses each .c with the include paths wasi-libc's own build
# uses, and reports the first error of each file. chibicc stops at the first
# error, so each line is one cause, not the only one.

set -u

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
libc="$root/wnjr-cc-libc"
cc="$root/wnjr-cc/wnjr-cc"
work=${TMPDIR:-/tmp}/wnjr-cc-libc-parse
mkdir -p "$work" || exit 1

make -C "$root/wnjr-cc" >/dev/null || { echo "FAIL: cannot build wnjr-cc"; exit 1; }

# The include path wasi-libc's build gives the compiler.
# musl's own sources come first, in musl's order: a source file includes
# <features.h> to get the internal one (which defines hidden, weak and the
# weak_alias machinery and then includes the public one), and it only does that
# if src/include is searched before include.
incs=(
  "-I$libc/libc-top-half/musl/src/include"
  "-I$libc/libc-top-half/musl/src/internal"
  "-I$libc/libc-top-half/musl/arch/wasm32"
  "-I$libc/libc-top-half/musl/arch/generic"
  "-I$libc/libc-top-half/musl/include"
  "-I$libc/libc-top-half/headers/private"
  "-I$libc/libc-bottom-half/headers/public"
  "-I$libc/libc-bottom-half/headers/private"
  "-I$libc/libc-bottom-half/cloudlibc/src"
  "-I$libc/libc-bottom-half/cloudlibc/src/include"
  "-I$libc/generated/include"
  "-I$libc/dlmalloc/include"
  "-I$libc/include"
)

verbose=0
[ "${1:-}" = "-v" ] && verbose=1

pass=0
total=0
: > "$work/fails"
for src in $(cd "$libc" && find . -name '*.c' | sort); do
  total=$((total + 1))
  name=${src#./}
  log="$work/${name//\//_}.log"
  if $cc --dump-ast "${incs[@]}" "$libc/$src" > /dev/null 2> "$log"; then
    pass=$((pass + 1))
    [ $verbose -eq 1 ] && echo "[ok]   $name"
  else
    # chibicc prints the source line, then a caret, then the message on the
    # caret's line; a #error has no caret and prints "file:line: error: ...".
    first=$(grep -m1 -e '^ *\^ ' "$log" | sed 's/^ *\^ //')
    [ -z "$first" ] && first=$(grep -m1 -e ' error: ' "$log" | sed 's/^[^:]*:[0-9]*: *//')
    [ -z "$first" ] && first=$(head -1 "$log")
    # Where it happened: the "file:line:" line chibicc prints above the source.
    # chibicc prints "file:line: <source line>" right above the caret.
    where=$(grep -m1 -B1 -e '^ *\^ ' "$log" | head -1 | sed -E 's/^([^:]*:[0-9]*):.*/\1/')
    where=$name"  ("$where")"
    echo "[fail] $where: $first" >> "$work/fails"
    [ $verbose -eq 1 ] && echo "[fail] $where: $first"
  fi
done

echo
echo "pass $pass / $total"
if [ $verbose -eq 0 ]; then
  echo
  cat "$work/fails"
fi
