// The difference of two pointers is a signed count of elements, and it is
// negative whenever the second sits above the first.
//
// Computed in the width of an address and widened afterwards, it loses its
// sign: "p - q" with p one element below q came out 1073741823 instead of -1,
// because the byte difference was zero-extended instead of subtracted in the
// 64 bits a ptrdiff_t has. musl's printf derives its decimal exponent from a
// pointer difference inside its own digit buffer, so "%g" of 0.5 then read
// digits from an address far outside that buffer and trapped. The regression
// that shows it from the outside is test/libc-io/float_fmt.c.
int neg_char(void) {
  char a[8];
  char *p = a, *q = a + 3;
  return p - q;
}

int pos_char(void) {
  char a[8];
  char *p = a + 3, *q = a;
  return p - q;
}

int neg_int(void) {
  int a[8];
  int *p = a, *q = a + 3;
  return p - q;
}

struct pair {
  long x, y;
};

int neg_wide(void) {
  struct pair a[4];
  struct pair *p = a, *q = a + 2;
  return p - q;
}

int tail(void) {
  int a[8];
  int *p = &a[7], *q = &a[0];
  return p - q;
}

int decay(void) {
  char *s = "hello";
  return (s + 1) - s + (s - (s + 4));
}
