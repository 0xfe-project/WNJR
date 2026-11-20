// Calling through a function pointer: taking a function's address, storing it
// in a global, in an array and in a struct, holding it only in a local,
// passing it as an argument, returning it from a function, and comparing two.

int add(int a, int b) { return a + b; }
int sub(int a, int b) { return a - b; }
int mul(int a, int b) { return a * b; }
int neg(int a) { return -a; }
void nothing(void) { }

int call2(int (*f)(int, int), int a, int b) { return f(a, b); }
int (*pick(int which))(int, int) {
  if (which == 0)
    return add;
  if (which == 1)
    return sub;
  return mul;
}
int apply_twice(int (*f)(int), int v) { return f(f(v)); }

// The four function-pointer slots of a file-scope object: a plain global, an
// array, a struct member and a static. All four are relocations in the data
// segment, not code.
int (*g_pick)(int, int) = mul;
int (*g_table[3])(int, int) = {add, sub, mul};
struct ops {
  int (*bin)(int, int);
  int (*un)(int);
};
struct ops g_ops = {sub, neg};
static int (*g_static)(int, int) = add;

int via_global(int a, int b) { return g_pick(a, b); }
int via_table(int i, int a, int b) { return g_table[i](a, b); }
int via_struct(int a, int b) { return g_ops.bin(a, b) + g_ops.un(1); }
int via_static(int a, int b) { return g_static(a, b); }
int via_local(int a, int b) {
  int (*f)(int, int) = add;
  return f(a, b);
}
int via_local_reassigned(int a, int b) {
  int (*f)(int, int) = add;
  f = sub;
  return f(a, b);
}
int via_returned(int which, int a, int b) { return pick(which)(a, b); }
int via_arg(int a, int b) { return call2(mul, a, b); }
int via_nested(int v) { return apply_twice(neg, v); }
int via_address_of(int a, int b) { return (&add)(a, b); }
int via_deref(int (*f)(int, int), int a, int b) { return (*f)(a, b); }
int via_deref_inner(int a, int b) { return via_deref(sub, a, b); }
int via_void(void) {
  void (*f)(void) = nothing;
  f();
  return 7;
}
int via_loop(int a, int b) {
  int (*f)(int, int) = add;
  int total = 0;
  for (int i = 0; i < 5; i++) {
    total = total + f(a, b);
    if (i == 2)
      f = sub;
  }
  return total;
}
int via_conditional(int flag, int a, int b) {
  int (*f)(int, int) = flag ? add : sub;
  return f(a, b);
}
int via_field_of_pointer(void) {
  struct ops o = {mul, neg};
  struct ops *p = &o;
  return p->bin(6, 7) + p->un(3);
}
int via_recursion(int (*f)(int, int), int n)
{
  if (n <= 1)
    return n;
  return f(n, via_recursion(f, n - 1));
}
int compare_pointers(void) {
  int (*f)(int, int) = add;
  int (*h)(int, int) = sub;
  if (f == h)
    return 1;
  if (f == add)
    return 2;
  if (f != h)
    return 3;
  return 4;
}
int table_roundtrip(void) {
  int total = 0;
  for (int i = 0; i < 3; i++)
    total = total + g_table[i](10, 4);
  return total;
}
int two_signatures(int a, int b) {
  // Two different signatures means two entries in the module's type list.
  int (*bin)(int, int) = sub;
  int (*un)(int) = neg;
  return bin(un(un(a)), b);
}
int via_recursion_inner(int a, int b) { return via_recursion(add, a) + via_recursion(sub, b); }
