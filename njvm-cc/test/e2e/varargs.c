// Variadic functions: the caller marshals its "..." arguments into a buffer of
// its own frame and passes that address as one more parameter, so a wasm
// signature stays fixed. va_start points a va_list at the buffer and every
// va_arg reads the next 8-byte slot.

#include <stdarg.h>

int sum_ints(int n, ...) {
  va_list ap;
  va_start(ap, n);
  int s = 0;
  for (int i = 0; i < n; i++)
    s += va_arg(ap, int);
  va_end(ap);
  return s;
}

// Twelve arguments, so the buffer is walked well past its first slots.
int twelve(int a, ...) {
  va_list ap;
  va_start(ap, a);
  int s = a;
  for (int i = 0; i < 11; i++)
    s = s * 3 + va_arg(ap, int);
  va_end(ap);
  return s;
}

// A float argument is promoted to double, and a double slot is read as one.
int doubles(int n, ...) {
  va_list ap;
  va_start(ap, n);
  double s = 0;
  for (int i = 0; i < n; i++)
    s += va_arg(ap, double);
  va_end(ap);
  return (int)(s * 10);
}

// The order and the widths have to survive together: an int slot, a double
// slot, a pointer slot, then a long.
int mixed(int n, ...) {
  va_list ap;
  va_start(ap, n);
  int i = va_arg(ap, int);
  double d = va_arg(ap, double);
  char *p = va_arg(ap, char *);
  long l = va_arg(ap, long);
  va_end(ap);
  return i * 1000 + (int)(d * 10) + (int)p[1] * 100000 + (int)l;
}

typedef struct {
  int a;
  int b;
} pair;

// A struct return goes through a hidden first parameter and the argument buffer
// is the last one, so the two conventions have to coexist in one signature.
pair pair_from(int n, ...) {
  va_list ap;
  va_start(ap, n);
  pair r;
  r.a = va_arg(ap, int);
  r.b = va_arg(ap, int);
  va_end(ap);
  return r;
}

// Passing a va_list to another function by value is what musl's vfprintf does;
// the callee's copy must not disturb the caller's position.
int vsum(int n, va_list ap) {
  int s = 0;
  for (int i = 0; i < n; i++)
    s += va_arg(ap, int);
  return s;
}

int sum_via_helper(int n, ...) {
  va_list ap;
  va_start(ap, n);
#ifdef __chibicc__
  int s = vsum(n, ap);
  // wcc passes the va_list by value: the caller's cursor is untouched by vsum's
  // copy, so this reads the first argument again, not the one after the last.
  // C leaves ap indeterminate here (7.16.1), so the host build cannot say.
#else
  va_list cp;
  va_copy(cp, ap);
  int s = vsum(n, cp);
  va_end(cp);
#endif
  s += va_arg(ap, int);
  va_end(ap);
  return s;
}

// va_copy has to give an independent position, which is the whole reason
// va_list is a struct here rather than a pointer.
int copy_independent(int n, ...) {
  va_list ap, ap2;
  va_start(ap, n);
  va_copy(ap2, ap);
  int first = va_arg(ap, int);
  int same_again = va_arg(ap2, int);
  int second = va_arg(ap, int);
  int pair_second = va_arg(ap2, int);
  va_end(ap);
  va_end(ap2);
  return first * 1000 + same_again * 100 + second * 10 + pair_second;
}

// Two live lists in one function: each va_start has its own storage, so the
// second must not rewind the first.
int two_lists(int n, ...) {
  va_list a, b;
  va_start(a, n);
  va_start(b, n);
  int x = va_arg(a, int);
  va_arg(b, int);
  int y = va_arg(a, int);
  int z = va_arg(b, int);
  va_end(a);
  va_end(b);
  return x * 100 + y * 10 + z;
}

// A return wider than 16 bytes is copied through the hidden buffer pointer, and
// the argument buffer pointer is the last parameter of the same signature.
typedef struct {
  char tag;
  long value;
  int nums[3];
} big;

big big_from(int n, ...) {
  va_list ap;
  va_start(ap, n);
  big b;
  b.tag = (char)va_arg(ap, int);
  b.value = va_arg(ap, long);
  b.nums[0] = va_arg(ap, int);
  b.nums[1] = va_arg(ap, int);
  b.nums[2] = va_arg(ap, int);
  va_end(ap);
  return b;
}

int big_from_inner(int a) { return big_from(5, a, 100L, 1, 2, 3).nums[2] * 1000 + a; }

typedef int (*variadic_fn)(int, ...);

// Two arguments through "...", so n is 2: the callee reads exactly what was
// passed.
int through_pointer(variadic_fn f, int n) { return f(2, 20, n); }

int nothing_through_ellipsis(int n, ...) {
  va_list ap;
  va_start(ap, n);
  va_end(ap);
  return n + 1;
}

// ── int wrappers, because the driver's call table carries only integers ──

int sum_ints_inner(int a) { return sum_ints(4, a, 10, 100, 1000); }
int twelve_inner(int a) { return twelve(a, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11); }
int doubles_inner(int a) { return doubles(3, 1.5, (double)a, 0.25); }
int mixed_inner(int a) {
  char s[4] = {'x', (char)a, 'z', 0};
  return mixed(1, 7, 2.5, s, 1234567890123L);
}
int pair_inner(int a) {
  pair p = pair_from(2, a, a + 5);
  return p.a * 100 + p.b;
}
int helper_inner(int a) { return sum_via_helper(3, a, 2, 3); }
int copy_inner(int a) { return copy_independent(2, a, 9); }
int two_lists_inner(int a) { return two_lists(2, a, a + 1); }
int pointer_inner(int a) { return through_pointer(sum_ints, a); }
int empty_inner(int a) { return nothing_through_ellipsis(a); }
