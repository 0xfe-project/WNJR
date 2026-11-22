// Codegen cases with no libc: arithmetic, control flow, pointers, arrays,
// structs, recursion, floats, switch, goto and computed goto.
//
// Every exported function takes only ints and returns int, so the harness can
// check it against the host C compiler as well as against WNJR.

int arith(int a, int b) {
  return (a + b) * 3 - a / 2 + (a % 5) - (b << 2) + (a ^ b) - ~b;
}

int compare(int a, int b) {
  int r = 0;
  if (a < b) r = r + 1;
  if (a <= b) r = r + 2;
  if (a == b) r = r + 4;
  if (a != b) r = r + 8;
  if (!(a > b)) r = r + 16;
  return r;
}

int loop_sum(int n) {
  int s = 0;
  int i;
  for (i = 0; i < n; i = i + 1) {
    if (i == 3) continue;
    if (i == 9) break;
    s = s + i;
  }
  return s;
}

int while_do(int n) {
  int s = 0;
  while (n > 0) { s = s + n; n = n - 1; }
  do { s = s * 2; n = n + 1; } while (n < 3);
  return s;
}

int fib(int n) {
  if (n < 2) return n;
  return fib(n - 1) + fib(n - 2);
}

int sw(int x) {
  int r;
  switch (x) {
  case 0: r = 10; break;
  case 1: r = 20; break;
  case 2:
  case 3: r = 30; break;
  case 7 ... 9: r = 40; break;
  default: r = 50; break;
  }
  return r;
}

int sw_fall(int x) {
  int r = 0;
  switch (x) {
  case 1: r = r + 1;
  case 2: r = r + 2; break;
  case 3: r = r + 4;
  default: r = r + 8;
  }
  return r;
}

int goto_loops(int n) {
  int s = 0;
again:
  if (n <= 0) goto done;
  s = s + n;
  n = n - 1;
  goto again;
done:
  return s;
}

int computed(int n) {
  void *p = &&two;
  if (n < 0) p = &&one;
  goto *p;
one:
  return 1;
two:
  return 2;
}

int computed_loop(int n) {
  void *table[3];
  table[0] = &&zero;
  table[1] = &&one;
  table[2] = &&two;
  int i = 0;
  int s = 0;
next:
  s = s + i;
  if (++i > n) goto done;
  goto *table[i % 3];
zero:
  goto next;
one:
  goto next;
two:
  goto next;
done:
  return s;
}

int array_sum(int n) {
  int a[8];
  int i;
  for (i = 0; i < 8; i = i + 1) a[i] = i * i;
  int s = 0;
  for (i = 0; i < n; i = i + 1) s = s + a[i];
  return s;
}

int pointer_walk(int n) {
  int a[6];
  int *p = a;
  int *q = a + 5;
  for (int i = 0; i < 6; i = i + 1) *(p + i) = i * 3;
  int s = 0;
  while (p <= q) { s = s + *p; p = p + 1; }
  return s * n;
}

struct pair {
  int a;
  int b;
};

int struct_ops(int x) {
  struct pair p;
  p.a = x;
  p.b = x * 2;
  struct pair *q = &p;
  q->a = q->a + 1;
  return p.a + p.b;
}

int max_of(int a, int b) {
  return a > b ? a : b;
}

int logic(int a, int b) {
  int r = 0;
  if (a > 0 && b > 0) r = r + 1;
  if (a > 0 || b > 0) r = r + 2;
  return r + (a && b) * 4 + (!a) * 8;
}

int charish(int n) {
  char c = n;
  unsigned char u = n * 3;
  short s = n * 100;
  long l = n * 1000;
  return c + u + s + (int)l;
}

int floatish(int n) {
  float f = n;
  double d = f * 1.5;
  return (int)d + (int)(d / 3.0);
}
