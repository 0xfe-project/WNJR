// wnjr-cc: align_to, moved out of chibicc's codegen.c (codegen is not ported).
// Based on chibicc by Rui Ueyama (MIT license, see LICENSE)

#include "chibicc.h"

// Round up `n` to the nearest multiple of `align`. For instance,
// align_to(5, 8) returns 8 and align_to(11, 8) returns 16.
int align_to(int n, int align) {
  return (n + align - 1) / align * align;
}
