// GCC's alias attribute: this declaration is another name for a function of the
// same file. The module exports the alias name on the same function index, so a
// call through either name reaches one body, and a function pointer taken
// through either name is the same pointer.
//
// The host compiler cannot check this: clang refuses the attribute on darwin
// ("aliases are not supported on darwin"), so this case carries expected
// results instead of comparing against a native build.

int target(int x) { return x * 3; }
int other_name(int x) __attribute__((__alias__("target")));

int via_alias(int a) { return other_name(a) + 1; }
int via_target(int a) { return target(a) + 1; }

// A function pointer taken through the alias has to reach the same body.
typedef int (*fn)(int);
int via_pointer_alias(int a) {
  fn f = other_name;
  fn g = target;
  return f(a) + g(a) * 10;
}

// An alias of an alias resolves through the chain.
int alias_of_alias(int x) __attribute__((__alias__("other_name")));
int via_chain(int a) { return alias_of_alias(a) + 100; }

// An alias can carry the name the module exports it under as well.
int renamed(int a) __attribute__((__alias__("target"), export_name("renamed_alias")));
