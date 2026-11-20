# The libc sources and include paths every wnjr-cc test program is built from.
#
# Two scripts need exactly this set — chibicc's suite and the libc's own probes —
# and a program that links a different libc than the other one takes its answers
# from is not the same program. So the list lives here once.
#
# Sets, for the caller:
#   $units  every libc unit that belongs to one program, absolute paths
#   $incs   the -I flags that reach the tree's headers, in the order the tree
#           resolves them (private before public, musl before cloudlibc)

libc_units() {
  libc=$1
  # The parts of the libc that are not part of one program, and why.
  #
  # dlmalloc/src/malloc.c is the allocator; dlmalloc/src/dlmalloc.c is the same
  # file plus this platform's configuration of it (#define HAVE_MMAP 0, and
  # MORECORE_CANNOT_TRIM, without which dlfree tries to shrink the heap and
  # sbrk aborts because a wasm memory cannot shrink). Compile the wrapper, not
  # the file it includes.
  skip_unit="libc-top-half/musl/src/env/__init_tls.c
libc-top-half/musl/src/thread/wasi-threads/pthread_create.c
dlmalloc/src/malloc.c
libc-bottom-half/crt/crt1-command.c
libc-bottom-half/sources/__main_void.c"

  units=""
  while IFS= read -r f; do
    p=${f#./}
    for s in $skip_unit; do [ "$p" = "$s" ] && continue 2; done
    units="$units $libc/$p"
  done < <(cd "$libc" && find . -name "*.c" | LC_ALL=C sort)
}

libc_incs() {
  libc=$1
  incs=""
  for d in libc-top-half/musl/src/include libc-top-half/musl/src/internal \
           libc-top-half/musl/arch/wasm32 libc-top-half/musl/arch/generic \
           libc-top-half/musl/include libc-top-half/headers/private \
           libc-bottom-half/headers/public libc-bottom-half/headers/private \
           libc-bottom-half/cloudlibc/src libc-bottom-half/cloudlibc/src/include \
           generated/include dlmalloc/include include; do
    incs="$incs -I$libc/$d"
  done
}
