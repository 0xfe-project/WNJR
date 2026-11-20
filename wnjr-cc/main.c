// wnjr-cc: C front-end for WNJR (modes: -E, --dump-ast, --emit-njvm)
// Based on chibicc by Rui Ueyama (MIT license, see LICENSE)

#include "chibicc.h"
#include "njvm.h"

StringArray include_paths;
bool opt_fcommon = true;
bool opt_fpic;
char *base_file;

static char *opt_o;
static bool opt_E;
static bool opt_dump_ast;
static bool opt_emit_njvm;

static void usage(int status) {
  fprintf(stderr, "wnjr-cc (-E | --dump-ast | --emit-njvm) [-o <path>] [-I<dir>] [-D<m>] <file>\n");
  exit(status);
}

bool file_exists(char *path) {
  struct stat st;
  return !stat(path, &st);
}

static void add_default_include_paths(char *argv0) {
  // wnjr-cc-specific include files in ./include relative to argv[0]
  strarray_push(&include_paths, format("%s/include", dirname(strdup(argv0))));

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

static void print_tokens(Token *tok) {
  FILE *out = open_file(opt_o);

  int line = 1;
  for (; tok->kind != TK_EOF; tok = tok->next) {
    if (line > 1 && tok->at_bol)
      fprintf(out, "\n");
    if (tok->has_space && !tok->at_bol)
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

  if (inputs.len == 0)
    error("no input files");
  if ((int)opt_E + (int)opt_dump_ast + (int)opt_emit_njvm != 1)
    error("exactly one of -E, --dump-ast or --emit-njvm is required");
  // -E and --dump-ast show what one file became; there is nothing to show for a
  // program made of several.
  if ((opt_E || opt_dump_ast) && inputs.len > 1)
    error("-E and --dump-ast take one file; %d were given", inputs.len);
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


  if (!base_file)
    base_file = inputs.data[0];

  add_default_include_paths(argv[0]);

  // The two modes that show one file's preprocessing or syntax stop at the first
  // (and only) unit.
  if (opt_E || opt_dump_ast) {
    start_unit();
    base_file = inputs.data[0];
    Token *tok = tokenize_file(inputs.data[0]);
    if (!tok)
      error("%s: cannot open file: %s", inputs.data[0], strerror(errno));
    tok = preprocess(tok);
    if (opt_E) {
      if (opt_o && !freopen(opt_o, "w", stdout))
        error("cannot open output file: %s: %s", opt_o, strerror(errno));
      print_tokens(tok);
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
