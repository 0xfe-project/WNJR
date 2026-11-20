// Two translation units compiled into one module. A name written in one and
// defined in the other resolves across the boundary; a file-scope static stays
// private to its unit even when both units use the same name for it.
//
// The other unit is multitu-b.c, listed in multitu.units.

int from_b(int);
extern int b_counter;
extern int b_calls_a(int);
extern int (*b_fnptr_a)(int); // an object in the other unit, not a function

int a_global = 100;
int shared_tent; // tentative here, defined in the other unit

static int pick(void) { return 7; }
static int both(void) { return 1; } // the same name as a static in the other unit

int tu_cross_call(int x) { return from_b(x) + 1; }

int tu_cross_global(int x) {
  b_counter += x;
  return b_counter;
}

// Each unit must see its own "both": this one returns 1, the other 2.
int tu_static_private(int x) { return both() * 10 + x; }

int tu_static_call(int x) { return pick() + x; }

int from_a(int x) { return x * 3; } // called back from the other unit

int tu_mutual(int x) { return b_calls_a(x) + 1; }

int tu_tentative(int x) {
  shared_tent += x;
  return shared_tent;
}

// The other unit holds a pointer to this object in its static initialiser.
int tu_other_pointer(int x) { return a_global + x; }

int tu_fnptr_to_other_unit(int x) { return b_fnptr_a(x); }

int tu_string_char(int i) {
  extern char *tu_string(void);
  return tu_string()[i];
}
