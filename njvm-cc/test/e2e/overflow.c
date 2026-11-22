// __builtin_{add,sub,mul}_overflow: the flag says whether the arithmetic
// wrapped and the pointer receives the wrapped result either way.
int add_flags(int a, int b) { int r; return __builtin_add_overflow(a, b, &r); }
int add_wrap(int a, int b) { int r = 0; __builtin_add_overflow(a, b, &r); return r; }
int sub_flags(int a, int b) { int r; return __builtin_sub_overflow(a, b, &r); }
int sub_wrap(int a, int b) { int r = 0; __builtin_sub_overflow(a, b, &r); return r; }
int mul_flags(int a, int b) { int r; return __builtin_mul_overflow(a, b, &r); }
int mul_wrap(int a, int b) { int r = 0; __builtin_mul_overflow(a, b, &r); return r; }
int uadd_flags(unsigned a, unsigned b) {
  unsigned r;
  return __builtin_add_overflow(a, b, &r);
}
int usub_flags(unsigned a, unsigned b) {
  unsigned r;
  return __builtin_sub_overflow(a, b, &r);
}
int umul_flags(unsigned a, unsigned b) {
  unsigned r;
  return __builtin_mul_overflow(a, b, &r);
}
int umul_wrap(unsigned a, unsigned b) {
  unsigned r = 0;
  __builtin_mul_overflow(a, b, &r);
  return (int)r;
}

// The same three at 64 bits, through int wrappers for the driver.
long ladd_flags(long a, long b) { long r; return __builtin_add_overflow(a, b, &r); }
long lmul_flags(long a, long b) { long r; return __builtin_mul_overflow(a, b, &r); }
int ladd_w(int a, int b) { return (int)ladd_flags(a, b); }
int lmul_w(int a, int b) { return (int)lmul_flags(a, b); }
