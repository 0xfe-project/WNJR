// The compiler builtins wasi-libc reaches for, each reduced to an operation the
// backend already has rather than to a call into a library that does not exist
// yet.

#ifndef __chibicc__
#ifndef __clang__
#define __builtin_assume(x) do { if (!(x)) __builtin_unreachable(); } while (0)
#endif
#endif

int clz32(int x) { return __builtin_clz(x); }
int ctz32(int x) { return __builtin_ctz(x); }
int pop32(unsigned int x) { return __builtin_popcount(x); }
int clz64(long x) { return __builtin_clzll(x); }
int ctz64(long x) { return __builtin_ctzll(x); }
int pop64(unsigned long x) { return __builtin_popcountll(x); }

int is_nan(double d) { return __builtin_isnan(d); }

// The value of the expression is the value of the first argument; the hint is
// parsed and dropped.
int expect_hint(int a) { return __builtin_expect(a * 2, 10); }
int constant_p_hint(int a) { return __builtin_constant_p(a) + a; }
int assume_hint(int a) { __builtin_assume(a >= 0); return a + 1; }

struct blob {
  int a;
  int b;
  char tail[4];
};

// memcpy and memset return the destination, and the destination expression is
// evaluated exactly once.
int memcpy_roundtrip(int seed) {
  struct blob src;
  src.a = seed;
  src.b = seed * 2;
  src.tail[0] = (char)seed;
  src.tail[1] = 7;
  src.tail[2] = 8;
  src.tail[3] = 9;

  struct blob dst;
  void *r = __builtin_memcpy(&dst, &src, sizeof dst);
  if (r != &dst)
    return -1;
  return dst.a + dst.b + dst.tail[0] + dst.tail[1] + dst.tail[2] + dst.tail[3];
}

int memset_bytes(void) {
  char buf[6];
  __builtin_memset(buf, 0x41, sizeof buf);
  int s = 0;
  for (int i = 0; i < 6; i++)
    s = s * 3 + buf[i];
  return s;
}

// memmove has the same lowering here: the engine's memory.copy is defined for
// overlapping ranges, so overlapping use is safe.
int memmove_overlap(int seed) {
  char buf[8];
  for (int i = 0; i < 8; i++)
    buf[i] = (char)(seed + i);
  __builtin_memmove(buf + 1, buf, 6);
  int s = 0;
  for (int i = 0; i < 8; i++)
    s = s * 3 + buf[i];
  return s;
}

int clz_inner(int a) { return clz32(a) * 100 + ctz32(a); }
int pop_inner(int a) { return pop32((unsigned)a) * 100 + is_nan(a); }
int wide_inner(int a) { return clz64(a) + ctz64((long)a * 8) + pop64((unsigned long)a); }
int hints_inner(int a) {
  return expect_hint(a) * 100 + constant_p_hint(a * 10) * 10 + assume_hint(a);
}
int memcpy_inner(int a) { return memcpy_roundtrip(a); }
int memset_inner(int a) { return memset_bytes() + a; }
int memmove_inner(int a) { return memmove_overlap(a); }
