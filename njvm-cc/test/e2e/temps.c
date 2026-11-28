// A temporary must survive as long as its value is still needed, and the codegen
// now hands temporaries back to be used again (temp_mark in codegen.c). The
// shapes that would break if one came back too early are all here: an assignment
// whose value is read after something else has been generated in between, a
// compound expression in a nested scope, a store into a member or an element
// whose address is computed before the value, and a condition with an assignment
// in it.
//
// Compared against the host compiler, so the expected numbers are not written
// here and cannot agree with a wrong answer.
static int n;

static int next(void) { return ++n; }

struct pair {
  int a, b;
};

int comma_value(void) {
  int a, b;
  a = (b = 3, b + 1);
  return a * 10 + b;
}

int element_and_index(void) {
  int t[4] = {0, 0, 0, 0};
  int i, j;
  for (i = 0; i < 4; i++)
    t[i] = (j = i, j * j);
  int s = 0;
  for (i = 0; i < 4; i++)
    s = s * 10 + t[i];
  return s;
}

int member_store(void) {
  struct pair p;
  p.a = next();
  p.b = p.a + next();
  return p.a * 10 + p.b;
}

int added_assignments(void) {
  int x = 0, y = 0;
  int r = (x = 5) + (y = x * 2);
  return r * 100 + x * 10 + y;
}

int stmt_expr(void) {
  int r = ({ int q = 3; q * 2; });
  int s = ({ int q = r + 1; q; });
  return r * 10 + s;
}

int loop_condition(void) {
  int a = 0, c, i = 0;
  while ((c = next()) < 4)
    a = a * 10 + c;
  return a * 10 + i;
}

int nested_call_args(void) {
  // The operands of + are unsequenced, so the host may evaluate them in any
  // order; sequence them with statements so both compilers must agree.
  int a = 0;
  int r = next() * 100;
  r += (a = next(), a * 10);
  r += next();
  return r;
}

int reset(int v) {
  n = 0;
  return v;
}
