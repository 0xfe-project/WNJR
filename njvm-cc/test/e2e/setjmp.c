// setjmp/longjmp through the wcc builtins __njvm_setjmp / __njvm_longjmp.
// The host build (no __chibicc__) uses the real <setjmp.h>, so the expected values are
// what C actually does. Covers: longjmp(v==0) returns 1, a jump out of 3 call
// levels, nested setjmp with the inner one dead, a loop re-entering the same
// setjmp site, jumping out of recursion, and locals set after setjmp.
#ifndef __chibicc__
#include <setjmp.h>
#else
typedef int jmp_buf[1];
int __njvm_setjmp(jmp_buf env);
_Noreturn void __njvm_longjmp(jmp_buf env, int val);
#define setjmp(env) __njvm_setjmp(env)
#define longjmp(env, val) __njvm_longjmp(env, val)
#endif

static jmp_buf outer, inner;
static int trace;

static void l3(int v) { trace = trace * 10 + 3; longjmp(outer, v); }
static void l2(int v) { trace = trace * 10 + 2; l3(v); trace = 99; }
static void l1(int v) { trace = trace * 10 + 1; l2(v); trace = 99; }

// 0 -> setjmp returns 0 first, then longjmp(…,0) must come back as 1.
int through_levels(int v) {
  trace = 0;
  int r = setjmp(outer);
  if (r == 0) { l1(v); return -1; }
  return r * 1000 + trace;
}

// Inner setjmp is jumped to; the outer one is never used.
int nested(int v) {
  int r = setjmp(outer);
  if (r != 0) return 500 + r;
  int s = setjmp(inner);
  if (s != 0) return 100 + s;
  longjmp(inner, v);
  return -1;
}

// Same setjmp site re-entered n times; the counter is static so it is well defined.
static int cnt, sum;  // not locals: a local changed after setjmp is indeterminate in C
int loop_static(int n) {
  sum = 0;
  cnt = 0;
  int r = setjmp(outer);
  sum += r;
  if (cnt < n) { cnt++; longjmp(outer, cnt * 2); }
  return sum * 100 + cnt;
}

static int depth_seen;
static int rec(int d, int limit) {
  depth_seen = d;
  if (d == limit) longjmp(outer, d + 7);
  return rec(d + 1, limit) + 1;
}
int from_recursion(int limit) {
  depth_seen = -1;
  int r = setjmp(outer);
  if (r == 0) return rec(0, limit);
  return r * 100 + depth_seen;
}

// A local written between setjmp and longjmp, volatile so its value is defined.
int volatile_local(int v) {
  volatile int x = 1;
  int r = setjmp(outer);
  if (r == 0) { x = x + 41; longjmp(outer, v); }
  return x * 10 + r;
}
