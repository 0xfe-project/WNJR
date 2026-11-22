// Thread-local storage. This runtime has one execution context, so the block
// __builtin_wasm_tls_base reports is the only one there is: a thread-local
// object behaves like a global whose initialiser is already in place, and its
// address is a constant. A second thread would copy the initial data somewhere
// else and point its own __tls_base at the copy.
//
// No host compiler has the three wasm intrinsics, so this case carries expected
// results instead of an oracle. Every function answers 0 or 1 except get.
_Thread_local int counter = 7;
_Thread_local long wide = 1234567890123;
int plain = 5;

int get(void) { return counter; }
void bump(void) { counter += 10; }
void reset(void) { counter = 7; }
int *addr(void) { return &counter; }
long wideget(void) { return wide; }
int same_as_plain(void) { return counter == plain; }

int wide_ok(void) { return wideget() == 1234567890123L; }
int addr_stable(void) { int *a = addr(); bump(); int v = *a; reset(); return v == 17; }

// The three intrinsics: the base is an address inside the data area, the size
// covers both thread-local objects, the alignment is a power of two, and both
// objects live inside the block.
int base_nonzero(void) { return __builtin_wasm_tls_base() != 0; }
int size_covers(void) { return __builtin_wasm_tls_size() >= 12; }
int align_pow2(void) {
  int a = __builtin_wasm_tls_align();
  return a > 0 && (a & (a - 1)) == 0;
}
int base_in_block(void) {
  char *b = (char *)(long)__builtin_wasm_tls_base();
  char *p = (char *)&counter;
  char *q = (char *)&wide;
  long size = __builtin_wasm_tls_size();
  return p >= b && p < b + size && q >= b && q < b + size;
}
int normal_is_outside(void) {
  char *b = (char *)(long)__builtin_wasm_tls_base();
  char *p = (char *)&plain;
  long size = __builtin_wasm_tls_size();
  return p < b || p >= b + size;
}

int nan_is_nan(void) { return __builtin_nanf("") != __builtin_nanf(""); }
int nan_compares_false(void) {
  float n = __builtin_nanf("");
  return n > 0 || n < 0 || n == n;
}
