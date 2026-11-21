// The parts of an initializer are joined with comma expressions, and a comma
// expression promises one thing: the parts happen in order. That chain is a
// balanced tree now (create_lvar_init), because a left-deep chain costs one
// stack frame per element, so the promise has to be checked rather than assumed:
// every element's expression bumps a counter, and the values have to come out in
// element order.
//
// C does not order an initializer list (6.7.9p23 says the order is unspecified),
// which is exactly why the reference is the host compiler this file is compared
// against: what matters is agreeing with the compiler a program was written for.
static int n;

static int bump(void) { return ++n; }

int array_order(void) {
  int t[5] = {bump(), bump(), bump(), bump(), bump()};
  int s = 0;
  for (int i = 0; i < 5; i++)
    s = s * 10 + t[i];
  return s;
}

struct pair {
  int a, b;
};

int struct_order(void) {
  struct pair t[2] = {{bump(), bump()}, {bump(), bump()}};
  int s = 0;
  for (int i = 0; i < 2; i++)
    s = s * 10 + t[i].a * 10 + t[i].b;
  return s;
}

int nested_order(void) {
  int t[2][3] = {{bump(), bump(), bump()}, {bump(), bump(), bump()}};
  int s = 0;
  for (int i = 0; i < 2; i++)
    for (int j = 0; j < 3; j++)
      s = s * 10 + t[i][j];
  return s;
}

int reset(int a) {
  n = 0;
  return a;
}
