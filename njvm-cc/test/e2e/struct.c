// Structs and unions by value: arguments, returns, assignment, nesting,
// recursion, and a struct crossing a function-pointer call.

struct pair {
  int a;
  int b;
};
struct big {
  char tag;
  long value;
  int nums[3];
};
struct outer {
  struct pair inner;
  int tail;
};
union either {
  int i;
  char c[4];
};

int sum(struct pair p) { return p.a + p.b; }
int sum_big(struct big b) { return (int)b.value + b.nums[0] + b.nums[1] + b.nums[2] + b.tag; }
int sum_outer(struct outer o) { return o.inner.a + o.inner.b + o.tail; }

struct pair make(int a, int b) {
  struct pair p;
  p.a = a;
  p.b = b;
  return p;
}
struct pair swap(struct pair p) {
  struct pair q = make(p.b, p.a);
  return q;
}
struct pair add_pairs(struct pair x, struct pair y) { return make(x.a + y.a, x.b + y.b); }

// A parameter is a copy: writing to it must not reach the caller's object.
int mutate_copy(struct pair p) {
  p.a = 100;
  p.b = 200;
  return p.a + p.b;
}
int copy_is_independent(int a, int b) {
  struct pair p = make(a, b);
  mutate_copy(p);
  return p.a + p.b;
}

int assign_copy(int a, int b) {
  struct pair x = make(a, b);
  struct pair y = make(0, 0);
  y = x;
  x.a = 7;
  return y.a + y.b;
}
int assign_chain(int a) {
  struct pair x = make(a, a + 1);
  struct pair y;
  struct pair z;
  z = y = x;
  return z.a + z.b + y.a + y.b;
}

int member_of_call(int a, int b) { return make(a, b).b; }
int member_of_returned_nested(void) {
  struct outer o;
  o.inner = make(3, 4);
  o.tail = 5;
  return o.inner.a * 100 + o.inner.b * 10 + o.tail;
}

int recurse_struct(struct pair p, int n) {
  if (n <= 0)
    return p.a + p.b;
  return recurse_struct(make(p.a + n, p.b - n), n - 1) + p.a;
}

struct pair combine(struct pair p) { return add_pairs(p, swap(p)); }

int sum_by_pointer(int (*f)(struct pair), struct pair p) { return f(p); }
int sum_by_pointer_inner(int a, int b) { return sum_by_pointer(sum, make(a, b)); }
struct pair make_via_pointer(int a, int b) {
  struct pair (*g)(int, int) = make;
  return g(a, b);
}
int call_through_two_levels(int a, int b) {
  struct pair (*g)(struct pair) = swap;
  struct pair p = make(a, b);
  struct pair q = g(p);
  return q.a * 10 + q.b;
}

int big_roundtrip(int seed) {
  struct big b;
  b.tag = 1;
  b.value = 1000 + seed;
  b.nums[0] = 10;
  b.nums[1] = 20;
  b.nums[2] = 30;
  struct big c = b;
  c.nums[1] = 0;
  return sum_big(c) + (b.nums[1] == 20 ? 0 : 100000);
}

int union_by_value(int v) {
  union either u;
  u.i = v;
  union either w = u;
  w.c[0] = 0;
  return w.i + (u.i == v ? 0 : 100000);
}

int array_in_struct(struct big b) {
  b.nums[2] = 99;
  return b.nums[0] + b.nums[1] + b.nums[2];
}
int array_in_struct_caller(int seed) {
  struct big b;
  b.tag = 0;
  b.value = seed;
  b.nums[0] = 1;
  b.nums[1] = 2;
  b.nums[2] = 3;
  int r = array_in_struct(b);
  return r * 10 + b.nums[2];
}
// The harness declares every call as int(int...), so anything with a struct in
// its signature is reached through an int wrapper here.
int sum_inner(int a, int b) { return sum(make(a, b)); }
int sum_big_inner(int seed) {
  struct big b;
  b.tag = 2;
  b.value = seed;
  b.nums[0] = 1;
  b.nums[1] = 2;
  b.nums[2] = 3;
  return sum_big(b);
}
int sum_outer_inner(int a, int b) {
  struct outer o;
  o.inner = make(a, b);
  o.tail = 9;
  return sum_outer(o);
}
int swap_inner(int a, int b) {
  struct pair p = swap(make(a, b));
  return p.a * 10 + p.b;
}
int add_pairs_inner(int a, int b) {
  struct pair p = add_pairs(make(a, b), make(a + 1, b + 1));
  return p.a * 100 + p.b;
}
int mutate_copy_inner(int a, int b) {
  struct pair p = make(a, b);
  return mutate_copy(p) + p.a * 1000 + p.b;
}
int recurse_struct_inner(int a, int n) {
  struct pair p = make(a, a + 1);
  return recurse_struct(p, n);
}
int combine_inner(int a, int b) {
  struct pair p = combine(make(a, b));
  return p.a * 100 + p.b;
}
int array_in_struct_inner(int seed) {
  struct big b;
  b.tag = 0;
  b.value = seed;
  b.nums[0] = 4;
  b.nums[1] = 5;
  b.nums[2] = 6;
  return array_in_struct(b) * 10 + b.nums[2];
}
int make_via_pointer_inner(int a, int b) {
  struct pair p = make_via_pointer(a, b);
  return p.a * 10 + p.b;
}
