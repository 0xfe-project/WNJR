// njvm-cc: C front-end for WNJR (modes: -E, --dump-ast, --emit-njvm)
// Based on chibicc by Rui Ueyama (MIT license, see LICENSE)

#include "chibicc.h"
#include "njvm.h"

StringArray include_paths;
StringArray deps;
bool opt_fcommon = true;
bool opt_fpic;
char *base_file;

static char *opt_o;
static bool opt_E;
static bool opt_dump_ast;
static bool opt_emit_njvm;
static bool opt_deps;
// Print, for every local variable of the unit, whether its address escapes. That
// answer is the only thing the escape analysis produces, so printing it is how
// the analysis is tested; codegen reads the same field.
static bool opt_dump_escapes;
static bool opt_dump_macros;
static bool opt_dump_include_paths;
static bool opt_emit_text;
bool opt_weld_tree;

static void usage(int status) {
  fprintf(stderr,
          "njvm-cc (-E | --dump-ast | --deps | --dump-macros | --emit-njvm)"
          " [-o <path>] [-I<dir>] [-D<m>] [--no-weld] <file>\n");
  exit(status);
}

bool file_exists(char *path) {
  struct stat st;
  return !stat(path, &st);
}

// The headers this compiler reads without being told: its own, then the libc it
// targets. wcc has one target and one libc, so the layout is a constant, not a
// set of flags. The libc tree sits beside the compiler's directory, which is
// what a checkout of this repository already looks like.
static void add_default_include_paths(char *argv0) {
  char *dir = dirname(strdup(argv0));

  // wcc's own headers win: they are the ones that agree with this front end
  // (stddef.h, and the stubs chibicc's suite expects to shadow the libc's).
  strarray_push(&include_paths, format("%s/include", dir));

  // The libc, in the order its own build reads it: private before public,
  // musl before cloudlibc. A directory that is not there is skipped, so an
  // installed compiler with no libc beside it still works.
  static const char *libc_dirs[] = {
    "libc-top-half/musl/src/include", "libc-top-half/musl/src/internal",
    "libc-top-half/musl/arch/wasm32", "libc-top-half/musl/arch/generic",
    "libc-top-half/musl/include", "libc-top-half/headers/private",
    "libc-bottom-half/headers/public", "libc-bottom-half/headers/private",
    "libc-bottom-half/cloudlibc/src", "libc-bottom-half/cloudlibc/src/include",
    "generated/include", "dlmalloc/include", "include",
  };
  for (unsigned i = 0; i < sizeof(libc_dirs) / sizeof(*libc_dirs); i++) {
    char *path = format("%s/../njvm-cc-libc/%s", dir, libc_dirs[i]);
    if (file_exists(path))
      strarray_push(&include_paths, path);
  }

  // Standard include paths (match chibicc ordering)
  strarray_push(&include_paths, "/usr/local/include");
  strarray_push(&include_paths, "/usr/include");
}

static FILE *open_file(char *path) {
  if (!path || strcmp(path, "-") == 0)
    return stdout;

  FILE *out = fopen(path, "w");
  if (!out)
    error("cannot open output file: %s: %s", path, strerror(errno));
  return out;
}

// separate puts a space between every pair of tokens. The welded table stores
// tokens as text and tokenizes that text again when a unit uses it, so a piece of
// text that tokenized into two tokens has to stay two: "static inline _Bool"
// printed without the space it never had becomes "static inline_Bool".
static void print_tokens(Token *tok, bool separate) {
  FILE *out = open_file(opt_o);

  int line = 1;
  for (; tok->kind != TK_EOF; tok = tok->next) {
    if (line > 1 && tok->at_bol)
      fprintf(out, "\n");
    if (separate || (tok->has_space && !tok->at_bol))
      fprintf(out, " ");
    fprintf(out, "%.*s", tok->len, tok->loc);
    line++;
  }
  fprintf(out, "\n");
  if (out != stdout)
    fclose(out);
}

// The macros given on the command line and the ones it undefines, in the order
// they were written. Every translation unit starts from the same set, so both
// are replayed before each one.
static StringArray cmdline_defs;
static StringArray cmdline_undefs;

static void apply_define(char *str) {
  char *eq = strchr(str, '=');
  weld_note_cmdline_macro(eq ? strndup(str, eq - str) : str);
  if (eq)
    define_macro(strndup(str, eq - str), eq + 1);
  else
    define_macro(str, "1");
}

static void define(char *str) {
  strarray_push(&cmdline_defs, str);
  apply_define(str);
}

// What every unit begins with: the predefined macros, then the command line.
static void start_unit(void) {
  init_macros();
  for (int i = 0; i < cmdline_defs.len; i++)
    apply_define(cmdline_defs.data[i]);
  for (int i = 0; i < cmdline_undefs.len; i++)
    undef_macro(cmdline_undefs.data[i]);
}

// Every .c on the command line is one translation unit of one program. A single
// unit is the common case; the wasi C library is a few hundred.
static StringArray inputs;

static void parse_args(int argc, char **argv) {
  for (int i = 1; i < argc; i++) {
    if (!strcmp(argv[i], "--help"))
      usage(0);

    if (!strcmp(argv[i], "-o")) {
      if (i + 1 >= argc)
        usage(1);
      opt_o = argv[++i];
      continue;
    }

    if (!strncmp(argv[i], "-o", 2)) {
      opt_o = argv[i] + 2;
      continue;
    }

    if (!strcmp(argv[i], "-E")) {
      opt_E = true;
      continue;
    }

    if (!strcmp(argv[i], "--dump-ast")) {
      opt_dump_ast = true;
      continue;
    }

    if (!strcmp(argv[i], "--emit-njvm")) {
      opt_emit_njvm = true;
      continue;
    }

    // The third way to stop early: say which files this unit depended on.
    if (!strcmp(argv[i], "--deps")) {
      opt_deps = true;
      continue;
    }

    if (!strcmp(argv[i], "--dump-escapes")) {
      opt_dump_escapes = true;
      continue;
    }

    // The macros a unit ends with, as #define lines. The welded table is
    // generated from this; nothing else asks for it.
    if (!strcmp(argv[i], "--dump-macros")) {
      opt_dump_macros = true;
      continue;
    }

    // Read every header from disk, as if nothing had been welded in. The tests
    // use it to compare the two paths on the same unit.
    if (!strcmp(argv[i], "--no-weld")) {
      weld_enabled = false;
      continue;
    }

    // -E, but with a space between every pair of tokens: the text the welded
    // table stores, which has to tokenize back into the tokens it came from.
    if (!strcmp(argv[i], "--emit-text")) {
      opt_emit_text = true;
      continue;
    }

    // With --emit-text: mark where each included file begins and ends. The
    // welded table is built from those boundaries.
    if (!strcmp(argv[i], "--weld-tree")) {
      opt_weld_tree = true;
      continue;
    }

    // Where this compiler looks for headers, in order. The generator of the
    // welded table needs to know: it welds a name from the directory that would
    // have answered it.
    if (!strcmp(argv[i], "--dump-include-paths")) {
      opt_dump_include_paths = true;
      continue;
    }

    if (!strcmp(argv[i], "-I")) {
      if (i + 1 >= argc)
        usage(1);
      strarray_push(&include_paths, argv[++i]);
      continue;
    }

    if (!strncmp(argv[i], "-I", 2)) {
      strarray_push(&include_paths, argv[i] + 2);
      continue;
    }

    if (!strcmp(argv[i], "-D")) {
      if (i + 1 >= argc)
        usage(1);
      define(argv[++i]);
      continue;
    }

    if (!strncmp(argv[i], "-D", 2)) {
      define(argv[i] + 2);
      continue;
    }

    if (!strcmp(argv[i], "-U")) {
      if (i + 1 >= argc)
        usage(1);
      strarray_push(&cmdline_undefs, argv[++i]);
      undef_macro(argv[i]);
      continue;
    }

    if (!strncmp(argv[i], "-U", 2)) {
      strarray_push(&cmdline_undefs, argv[i] + 2);
      undef_macro(argv[i] + 2);
      continue;
    }

    if (!strcmp(argv[i], "-cc1-input")) {
      if (i + 1 >= argc)
        usage(1);
      base_file = argv[++i];
      continue;
    }

    if (argv[i][0] == '-' && argv[i][1] != '\0')
      error("unknown argument: %s", argv[i]);

    strarray_push(&inputs, argv[i]);
  }

  if (inputs.len == 0 && !opt_dump_include_paths)
    error("no input files");
  // Emitting the module is what a compiler does; -E and --dump-ast are the two
  // ways to ask it to stop early instead. So a bare "wcc a.c -o a.bin" needs no
  // mode word, and --emit-njvm is still accepted for a reader who wants to say
  // it out loud.
  if (!opt_E && !opt_dump_ast && !opt_deps && !opt_dump_macros &&
      !opt_dump_include_paths && !opt_emit_text && !opt_dump_escapes)
    opt_emit_njvm = true;
  if ((int)opt_E + (int)opt_dump_ast + (int)opt_deps + (int)opt_dump_macros +
        (int)opt_dump_include_paths + (int)opt_emit_text +
        (int)opt_dump_escapes + (int)opt_emit_njvm != 1)
    error("exactly one of -E, --dump-ast, --deps, --dump-macros, --dump-escapes"
          " or --emit-njvm is required");
  // The inspection modes show what one file became; there is nothing to show for
  // a program made of several.
  if ((opt_E || opt_emit_text || opt_dump_ast || opt_deps || opt_dump_macros ||
       opt_dump_escapes) && inputs.len > 1)
    error("-E, --dump-ast, --deps, --dump-macros and --dump-escapes take one"
          " file; %d were given", inputs.len);
}

// The escape answer for every local variable of every function in the unit. A
// variable that says "value" is one whose address nothing took, so code
// generation may hold it in a register instead of a frame slot; "escapes" is
// everything else, including the objects the compiler puts in the frame itself.
//
// This is the analysis' only output, so this is what tests it: the names in the
// list, and which side of that line each one lands on.
static void dump_escapes(Obj *prog, FILE *out) {
  for (Obj *fn = prog; fn; fn = fn->next) {
    if (!fn->is_function || !fn->body)
      continue;
    fprintf(out, "function %s\n", fn->name);
    for (Obj *var = fn->locals; var; var = var->next)
      fprintf(out, "  %s %s\n", var->name[0] ? var->name : "<temporary>",
              var->in_frame ? "frame" : "register");
  }
}

// Links a unit's object list after `tail` and returns the tail of the result, so
// that the next unit follows this one. Order is the order the units were given
// on the command line; nothing depends on it except the addresses in the data
// area.
static Obj *append_program(Obj *tail, Obj *unit) {
  if (!unit)
    return tail;
  if (tail)
    tail->next = unit;
  while (unit->next)
    unit = unit->next;
  return unit;
}

int main(int argc, char **argv) {
  init_macros();
  parse_args(argc, argv);
  // The command line is replayed per unit by start_unit; what parse_args did is
  // only for the modes that never reach a unit.


  add_default_include_paths(argv[0]);

  // The generator of the welded table (tools/scripts/gen_libc_headers.py) has to
  // walk exactly the directories this compiler searches, in this order: a name
  // welded from the wrong directory would resolve differently from the file it
  // replaces.
  if (opt_dump_include_paths) {
    for (int i = 0; i < include_paths.len; i++)
      fprintf(stdout, "%s\n", include_paths.data[i]);
    return 0;
  }

  if (!base_file)
    base_file = inputs.data[0];

  // The two modes that show one file's preprocessing or syntax stop at the first
  // (and only) unit.
  if (opt_E || opt_emit_text || opt_dump_ast || opt_deps || opt_dump_macros ||
      opt_dump_escapes) {
    start_unit();
    base_file = inputs.data[0];
    record_dep(inputs.data[0]);
    Token *tok = tokenize_file(inputs.data[0]);
    if (!tok)
      error("%s: cannot open file: %s", inputs.data[0], strerror(errno));
    tok = preprocess(tok);
    if (opt_dump_macros) {
      FILE *out = open_file(opt_o);
      dump_macros(out);
      if (out != stdout)
        fclose(out);
      return 0;
    }
    if (opt_deps) {
      FILE *out = open_file(opt_o);
      for (int i = 0; i < deps.len; i++)
        fprintf(out, "%s\n", deps.data[i]);
      if (out != stdout)
        fclose(out);
      return 0;
    }
    if (opt_E || opt_emit_text) {
      if (opt_o && !freopen(opt_o, "w", stdout))
        error("cannot open output file: %s: %s", opt_o, strerror(errno));
      print_tokens(tok, opt_emit_text);
      return 0;
    }
    if (opt_dump_escapes) {
      FILE *out = open_file(opt_o);
      dump_escapes(parse(tok, 0), out);
      if (out != stdout)
        fclose(out);
      return 0;
    }
    FILE *out = open_file(opt_o);
    dump_ast(parse(tok, 0), out);
    if (out != stdout)
      fclose(out);
    return 0;
  }

  // Preprocess and parse every unit, splicing them into one program. The
  // preprocessing state is per unit (macros are), so it runs per unit.
  Obj *prog = NULL;
  Obj *tail = NULL;
  for (int i = 0; i < inputs.len; i++) {
    start_unit();
    base_file = inputs.data[i];
    record_dep(inputs.data[i]);
    Token *tok = tokenize_file(inputs.data[i]);
    if (!tok)
      error("%s: cannot open file: %s", inputs.data[i], strerror(errno));
    tok = preprocess(tok);
    Obj *unit = parse(tok, i);
    if (!prog)
      prog = unit;
    tail = append_program(tail, unit);
  }

  if (opt_emit_njvm) {
    Mp mp;
    mp_init(&mp);
    njvm_emit_module(prog, &mp);
    FILE *out = stdout;
    if (opt_o) {
      out = fopen(opt_o, "wb");
      if (!out)
        error("cannot open output file: %s: %s", opt_o, strerror(errno));
    }
    mp_write_file(&mp, out);
    if (out != stdout)
      fclose(out);
    mp_free(&mp);
    return 0;
  }

  return 0;
}
