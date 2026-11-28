// Variable-length arrays. sizeof of one is computed at run time, and the array
// itself is a region carved out of the stack at its declaration, so these check
// the size arithmetic, the region's contents, and that ordinary locals keep
// their values while the stack pointer moves.
//
// A VLA declared inside a loop is a stack leak in this model (nothing restores
// the pointer until the function returns); C requires neither, and nothing in
// the C library does it.

int sizeof_1d(void) {
  int n = 5;
  int x[n];
  return sizeof(x);
}

int sizeof_2d(void) {
  int m = 5, n = 8;
  int x[m + 1][n * 2];
  return sizeof(x);
}

// Both dimensions follow the same variable, so each row's size is itself
// computed once (compute_vla_size) instead of once per row.
int sizeof_2d_same(void) {
  int n = 3;
  int x[n][n];
  return sizeof(x);
}

int sizeof_elem(void) {
  int n = 3;
  int x[5][n];
  return sizeof(*x);
}

int fill_2d(void) {
  int n = 10;
  int x[n + 1][n + 6];
  int *p = (int *)x;
  for (int i = 0; i < sizeof(x) / 4; i++)
    p[i] = i;
  return x[5][2];
}

int fill_2d_first(void) {
  int n = 10;
  int x[n + 1][n + 6];
  int *p = (int *)x;
  for (int i = 0; i < sizeof(x) / 4; i++)
    p[i] = i;
  return x[0][5];
}

// The size comes from a parameter, so it is not known until the call.
int from_param(int n) {
  int x[n];
  for (int i = 0; i < n; i++)
    x[i] = i * i;
  int s = 0;
  for (int i = 0; i < n; i++)
    s += x[i];
  return s;
}

// Locals declared on both sides of the VLA, read after it has been written.
int locals_survive(int n) {
  int a = 11, b = 22;
  int x[n];
  for (int i = 0; i < n; i++)
    x[i] = i;
  int s = 0;
  for (int i = 0; i < n; i++)
    s += x[i];
  return a * 1000 + b * 10 + (s > 0);
}

// Two regions must not overlap.
int two_vlas(int n) {
  int a[n];
  int b[n];
  for (int i = 0; i < n; i++) {
    a[i] = i;
    b[i] = i * 2;
  }
  int s = 0;
  for (int i = 0; i < n; i++)
    s += a[i] + b[i];
  return s;
}

// char-sized elements, so the region is not a multiple of the alignment.
int bytes(int n) {
  char x[n + 1];
  for (int i = 0; i < n; i++)
    x[i] = (char)(i + 1);
  x[n] = 0;
  int s = 0;
  for (int i = 0; i < n; i++)
    s += x[i];
  return s;
}
