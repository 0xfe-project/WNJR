// Objects with a static extent: globals (initialised and not), string
// literals, static locals, an array of pointers to string literals, and a
// global struct. No libc, so the string helpers are written out.

int g_init = 7;
int g_zero;
int g_arr[4] = {1, 2, 3, 4};
char g_str[] = "hello";
char *g_ptr = g_str + 1;
int *g_addr = &g_zero;
struct Pair { int a; int b; };
struct Pair g_pair = {11, 22};
float g_f = 0.5f;
double g_d = 2.25;
int g_back;

int read_init(void) { return g_init; }
int bump_init(void) { g_init = g_init + 1; return g_init; }
int read_zero(void) { return g_zero; }
int write_zero(int v) { g_zero = v; return g_zero; }
int sum_arr(void) { int s = 0; for (int i = 0; i < 4; i++) s = s + g_arr[i]; return s; }
int grow_arr(int d) { g_arr[2] = g_arr[2] + d; return g_arr[2]; }
int str_len(char *s) { int n = 0; while (s[n]) n = n + 1; return n; }
int str_lit_len(void) { return str_len("abcdef"); }
int gstr_len(void) { return str_len(g_str); }
int gptr_len(void) { return str_len(g_ptr); }
int via_addr(void) { *g_addr = 5; return g_zero; }
int pair_sum(void) { return g_pair.a + g_pair.b; }
int pair_set(int v) { g_pair.b = v; return g_pair.a + g_pair.b; }
int float_bits(void) { return (int)(g_f * 4.0f); }
int double_bits(void) { return (int)(g_d * 4.0); }
int counter(void) { static int c = 0; c = c + 1; return c; }
int counter_twice(void) { return counter() * 10 + counter(); }
char *name_at(int i) {
  static char *names[3] = {"a", "bb", "ccc"};
  if (i < 0 || i > 2) return "";
  return names[i];
}
int name_len(int i) { return str_len(name_at(i)); }
int global_addr_roundtrip(void) { g_back = (int)g_addr; return *(int *)g_back; }
