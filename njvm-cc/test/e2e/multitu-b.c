// The second unit of the multitu case. See multitu.c.

extern int a_global;
extern int from_a(int);
int b_calls_a(int);
char *tu_string(void);

static int both(void) { return 2; }
static int b_static_only = 41;

int b_counter = 5;
int shared_tent = 9; // the definition; the other unit only declares it

int from_b(int x) { return both() * 100 + x + b_static_only + b_counter; }

int b_calls_a(int x) { return from_a(x) + 1; }

// Static initialisers that name a function and an object: one here, one in the
// other unit.
int (*b_fnptr)(int) = from_b;
int (*b_fnptr_a)(int) = from_a;
int *ap = &a_global;

int tu_call_through_pointer(int x) { return b_fnptr(x); }

int tu_pointer_to_other_unit(int x) {
  *ap += x;
  return *ap;
}

char *tu_string(void) { return "from b"; }
