// The C99 classification macros. Each is a property of the bit pattern, so the
// test builds the value from its two halves: the driver declares every call
// int(int, ...) and could not carry a double otherwise.
union dbits { int i[2]; double d; };
union fbits { int i; float f; };

static double d_of(int lo, int hi) {
  union dbits u;
  u.i[0] = lo;
  u.i[1] = hi;
  return u.d;
}

static float f_of(int bits) {
  union fbits u;
  u.i = bits;
  return u.f;
}

int isinf_d(int lo, int hi) { return __builtin_isinf(d_of(lo, hi)) != 0; }
int isinf_f(int bits) { return __builtin_isinf(f_of(bits)) != 0; }
int finite_d(int lo, int hi) { return __builtin_isfinite(d_of(lo, hi)) != 0; }
int finite_f(int bits) { return __builtin_isfinite(f_of(bits)) != 0; }
int normal_d(int lo, int hi) { return __builtin_isnormal(d_of(lo, hi)) != 0; }
int signd_d(int lo, int hi) { return __builtin_signbit(d_of(lo, hi)) != 0; }
int signd_f(int bits) { return __builtin_signbit(f_of(bits)) != 0; }
// Two operands are four ints, so these three take all four.
int unordered4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_isunordered(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
int lessgreater4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_islessgreater(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
int isless4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_isless(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
int isgreater4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_isgreater(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
int isgreaterequal4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_isgreaterequal(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
int islessequal4(int lo1, int hi1, int lo2, int hi2) {
  return __builtin_islessequal(d_of(lo1, hi1), d_of(lo2, hi2)) != 0;
}
