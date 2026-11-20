// The compiler's data model and the ABI's own types have to agree on where the
// line is, because C code cannot see that they disagree: a size_t of the wrong
// width still type-checks everywhere, and the first thing that notices is a
// library doing arithmetic with it. dlmalloc is that library — its sanity check
// reads sizeof(size_t) != sizeof(char*) and aborts, which cost an afternoon
// chasing a trap inside malloc that was really a wrong typedef two headers away.
//
// The rule these numbers state: in C a size is the pointer width (eight bytes,
// LP64, so a pointer and an offset are the same word), while a WASI length is
// four bytes because that is what the ABI's structures are laid out for. The
// predefine __SIZEOF_SIZE_T__ has to say the same thing as sizeof(size_t), or
// the two disagree in the one place nobody re-reads.
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <wasi/api.h>

int main(void) {
  int fails = 0;

  if (sizeof(size_t) != 8) fails++;
  if (sizeof(ssize_t) != 8) fails++;
  if (sizeof(ptrdiff_t) != 8) fails++;
  if (sizeof(uintptr_t) != 8) fails++;
  if (sizeof(void *) != 8) fails++;
  // A size and a pointer have to be the same width, which is the invariant the
  // allocator leans on.
  if (sizeof(size_t) != sizeof(void *)) fails++;
  // The predefine and the type must not drift apart.
  if (__SIZEOF_SIZE_T__ != sizeof(size_t)) fails++;
  if (__SIZEOF_POINTER__ != sizeof(void *)) fails++;
  // The ABI's types keep their own widths.
  if (sizeof(__wasi_size_t) != 4) fails++;
  if (sizeof(__wasi_iovec_t) != 8) fails++;
  // A tag lives in the upper half of a pointer, so a pointer converted to
  // uintptr_t and back has to survive it.
  if (sizeof(uintptr_t) != sizeof(void *) ) fails++;

  printf("size_t=%d ssize_t=%d ptrdiff_t=%d uintptr_t=%d ptr=%d\n", (int)sizeof(size_t),
         (int)sizeof(ssize_t), (int)sizeof(ptrdiff_t), (int)sizeof(uintptr_t), (int)sizeof(void *));
  printf("predefined_size_t=%d predefined_ptr=%d\n", (int)__SIZEOF_SIZE_T__, (int)__SIZEOF_POINTER__);
  printf("wasi_size_t=%d wasi_iovec_t=%d\n", (int)sizeof(__wasi_size_t), (int)sizeof(__wasi_iovec_t));
  printf("fails=%d\n", fails);
  return fails;
}
