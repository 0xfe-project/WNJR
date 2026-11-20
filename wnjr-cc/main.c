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

static void define(char *str) {
  char *eq = strchr(str, '=');
  if (eq)
    define_macro(strndup(str, eq - str), eq + 1);
  else
    define_macro(str, "1");
}

static char *input_path;

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
      undef_macro(argv[++i]);
      continue;
    }

    if (!strncmp(argv[i], "-U", 2)) {
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

    input_path = argv[i];
  }

  if (!input_path)
    error("no input files");
  if ((int)opt_E + (int)opt_dump_ast + (int)opt_emit_njvm != 1)
    error("exactly one of -E, --dump-ast or --emit-njvm is required");
}

int main(int argc, char **argv) {
  init_macros();
  parse_args(argc, argv);

  if (!base_file)
    base_file = input_path;

  add_default_include_paths(argv[0]);

  Token *tok = tokenize_file(input_path);
  if (!tok)
    error("%s: cannot open file: %s", input_path, strerror(errno));
  tok = preprocess(tok);

  if (opt_emit_njvm) {
    Obj *prog = parse(tok);
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

  if (opt_o && !freopen(opt_o, "w", stdout))
    error("cannot open output file: %s: %s", opt_o, strerror(errno));

  if (opt_E) {
    print_tokens(tok);
    return 0;
  }

  Obj *prog = parse(tok);
  FILE *out = open_file(opt_o);
  dump_ast(prog, out);
  if (out != stdout)
    fclose(out);
  return 0;
}
