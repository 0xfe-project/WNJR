#!/bin/bash
# How much of njvm-cc-libc (the trimmed wasi-libc) can njvm-cc get through?
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
libc="$root/njvm-cc-libc"
cc="$root/njvm-cc/wcc"
work=${TMPDIR:-/tmp}/njvm-cc-libc-parse
mkdir -p "$work" || exit 1

make -C "$root/njvm-cc" >/dev/null || { echo "FAIL: cannot build wcc"; exit 1; }

# No include paths and no definitions here: wcc knows the libc it targets, which
# sits beside it, and it knows the target has no mmap. What is left on the
# command line is the file.

# Files this frontend will not take, each with the reason it will not. They are
# listed on every run and counted apart from the pass rate: a file that cannot be
# compiled must never quietly leave the denominator.
skip_reason() {
  case "$1" in
    libc-top-half/musl/src/env/__init_tls.c)
      echo "single-threaded runtime: the TLS block is a link-time constant and is never copied, so there is nothing to initialise; and the file reaches that conclusion through inline asm (global.get __stack_pointer, local.set %0), which this frontend does not implement";;
    libc-top-half/musl/src/thread/wasi-threads/pthread_create.c)
      echo "wasi-threads build only: no _REENTRANT in a single-threaded libc, and __thread_list_lock is defined in a file that is not in this tree";;
    *) return 1;;
  esac
}

verbose=0
[ "${1:-}" = "-v" ] && verbose=1

pass=0
total=0
skipped=0
: > "$work/fails"
for src in $(cd "$libc" && find . -name '*.c' | sort); do
  name=${src#./}
  if reason=$(skip_reason "$name"); then
    skipped=$((skipped + 1))
    echo "[skip] $name: $reason" >> "$work/fails"
    continue
  fi
  total=$((total + 1))
  log="$work/${name//\//_}.log"
  if $cc --dump-ast "$libc/$src" > /dev/null 2> "$log"; then
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
echo "pass $pass / $total (and $skipped listed as not supported)"
if [ $verbose -eq 0 ]; then
  echo
  cat "$work/fails"
fi
