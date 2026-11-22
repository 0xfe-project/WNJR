#define _POSIX_C_SOURCE 200809L
#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <libgen.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdnoreturn.h>
#include <string.h>
#include <strings.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#define MAX(x, y) ((x) < (y) ? (y) : (x))
#define MIN(x, y) ((x) < (y) ? (x) : (y))

#ifndef __GNUC__
# define __attribute__(x)
#endif

typedef struct Type Type;
typedef struct Node Node;
typedef struct Member Member;
typedef struct Relocation Relocation;
typedef struct Hideset Hideset;

//
// strings.c
//

typedef struct {
  char **data;
  int capacity;
  int len;
} StringArray;

void strarray_push(StringArray *arr, char *s);
char *format(char *fmt, ...) __attribute__((format(printf, 1, 2)));

//
// tokenize.c
//

// Token
typedef enum {
  TK_IDENT,   // Identifiers
  TK_PUNCT,   // Punctuators
  TK_KEYWORD, // Keywords
  TK_STR,     // String literals
  TK_NUM,     // Numeric literals
  TK_PP_NUM,  // Preprocessing numbers
  TK_EOF,     // End-of-file markers
} TokenKind;

typedef struct {
  char *name;
  int file_no;
  char *contents;

  // For #line directive
  char *display_name;
  int line_delta;
} File;

// Token type
typedef struct Token Token;
struct Token {
  TokenKind kind;   // Token kind
  Token *next;      // Next token
  int64_t val;      // If kind is TK_NUM, its value
  long double fval; // If kind is TK_NUM, its value
  char *loc;        // Token location
  int len;          // Token length
  Type *ty;         // Used if TK_NUM or TK_STR
  char *str;        // String literal contents including terminating '\0'

  File *file;       // Source location
  char *filename;   // Filename
  int line_no;      // Line number
  int line_delta;   // Line number
  bool at_bol;      // True if this token is at beginning of line
  bool has_space;   // True if this token follows a space character
  Hideset *hideset; // For macro expansion
  Token *origin;    // If this is expanded from a macro, the original token
  // True for a token that came out of a welded header (weld.h), which is text
  // produced by this compiler's own preprocessor at build time. Such a token is
  // already expanded, so preprocess2 must neither expand it as a macro nor read
  // it as a directive: a macro defined after the header was welded would
  // otherwise rewrite a name the header had already resolved.
  bool is_welded;
};

noreturn void error(char *fmt, ...) __attribute__((format(printf, 1, 2)));
noreturn void error_at(char *loc, char *fmt, ...) __attribute__((format(printf, 2, 3)));
noreturn void error_tok(Token *tok, char *fmt, ...) __attribute__((format(printf, 2, 3)));
void warn_tok(Token *tok, char *fmt, ...) __attribute__((format(printf, 2, 3)));
bool equal(Token *tok, char *op);
Token *skip(Token *tok, char *op);
bool consume(Token **rest, Token *tok, char *str);
void convert_pp_tokens(Token *tok);
File **get_input_files(void);
File *new_file(char *name, int file_no, char *contents);
Token *tokenize_string_literal(Token *tok, Type *basety);
Token *tokenize(File *file);
Token *tokenize_file(char *filename);
// Tokenize text this compiler carries rather than a file on disk: the same
// canonicalisation a file gets, without reading one.
Token *tokenize_text(char *name, char *contents);

#define unreachable() \
  error("internal error at %s:%d", __FILE__, __LINE__)

//
// preprocess.c
//

char *search_include_paths(char *filename);
void init_macros(void);
void define_macro(char *name, char *buf);
void undef_macro(char *name);
bool macro_defined(char *name);
// Tell the preprocessor that the command line named a macro: a name beginning
// with "_" is how a program asks a header for a different configuration, and a
// welded header was expanded under the default one.
void weld_note_cmdline_macro(char *name);
// Every macro a unit ends with, as the "#define" lines that would recreate it.
// The generator of the welded table uses this to learn what a header leaves
// behind; nothing else does.
void dump_macros(FILE *out);
Token *preprocess(Token *tok);

// Whether an #include may be answered from the welded table (weld.h) instead of
// opening the file. --no-weld turns it off, and the tests use that to compare
// the two paths.
extern bool weld_enabled;
// Whether to mark where each included file begins and ends in the output. The
// generator of the welded table needs the boundary: the table stores what a
// header adds on its own, which is its text minus the text of the headers it
// includes, and nothing in a flat token stream says where one ends.
extern bool opt_weld_tree;
// How many includes this run answered from the table, and how many it declined
// because the unit had already defined a macro of its own.
extern int weld_hits;
extern int weld_misses;

//
// parse.c
//

// Variable or function
typedef struct Obj Obj;
struct Obj {
  Obj *next;
  char *name;    // Variable name
  Type *ty;      // Type
  Token *tok;    // representative token
  bool is_local; // local or global/function
  int align;     // alignment

  // Local variable
  int offset;
  // Set when this variable has to keep a place in the frame instead of living in
  // a wasm local -- which is a register. Three reasons put it there: its address
  // is taken anywhere in the function (whatever took it keeps pointing at that
  // memory), its type is not one machine word that can be read and written whole,
  // or it is a frame object codegen addresses directly.
  // Decided once on the AST, by mark_escapes() in parse.c.
  bool in_frame;
  // The wasm local that holds this variable instead of a frame slot, or -1 when
  // it still lives in the frame. Assigned by emit_function in codegen.c, which
  // is the only place that knows a function's local numbering.
  int promoted;
  // Set on a function that calls alloca, or declares a variable-length array --
  // which is lowered to alloca. Such a function moves the stack pointer below its
  // own frame, so it must keep one even when every local of its lives in a
  // register; the alternative is a stack pointer that never comes back.
  bool uses_alloca;

  // Global variable or function
  bool is_function;
  bool is_definition;

  // Which translation unit of the program this object came from. A file-scope
  // static sees only its own unit, so its name is only unique there.
  int tu;
  // The next object with the same name, in the program-wide symbol table. Two
  // entries share a name when one is the definition and the other a declaration,
  // or when each belongs to a different unit and both are static.
  Obj *name_next;

  // Where the host function that stands in for this one is filed, when the
  // declaration carries wasi's import attribute. See parse.c:import_attribute.
  // A function with these set is not defined in this file and its call goes to
  // the imported function.
  char *import_module;
  char *import_name;
  // The name this function is exported under, when it is not its own.
  char *export_name;
  // Set for a declaration that carries GCC's alias attribute: this name is
  // another name for another function of this file, and has the same index.
  char *alias_target;
  bool is_alias;
  // Set for a symbol the compiler itself provides rather than the program
  // (__heap_base, __heap_end, __wasm_call_ctors). It is a definition, every unit
  // sees it by name, and it is never exported.
  bool is_provided;
  // Set for the "alloca" that parse.c inserts for a variable-length array and
  // that __builtin_alloca names. It has no body and no import: codegen emits the
  // allocation itself, because it is a stack-pointer move, not a call.
  bool is_alloca_builtin;
  bool is_static;

  // Global variable
  bool is_tentative;
  bool is_tls;
  char *init_data;
  Relocation *rel;

  // Function
  bool is_inline;
  Obj *params;
  Node *body;
  Obj *locals;
  Obj *va_area;
  // Where a variadic function keeps the argument buffer pointer the caller
  // passed as its last parameter. See include/stdarg.h.
  Obj *va_buf;
  Obj *alloca_bottom;
  int stack_size;

  // Static inline function
  bool is_live;
  bool is_root;
  StringArray refs;
};

// Global variable can be initialized either by a constant expression
// or a pointer to another global variable. This struct represents the
// latter.
typedef struct Relocation Relocation;
struct Relocation {
  Relocation *next;
  int offset;
  char **label;
  long addend;
  // The function whose computed-goto label this word names, when the
  // initialiser named one ("static void *p[] = {&&a};"). A label number only
  // means something inside its own function, so the fixup has to say which,
  // and it is the enclosing function of the declaration being initialised.
  // NULL for an ordinary object or function address.
  Obj *owner;
};

// AST node
typedef enum {
  ND_NULL_EXPR, // Do nothing
  ND_ADD,       // +
  ND_SUB,       // -
  ND_MUL,       // *
  ND_DIV,       // /
  ND_NEG,       // unary -
  ND_MOD,       // %
  ND_BITAND,    // &
  ND_BITOR,     // |
  ND_BITXOR,    // ^
  ND_SHL,       // <<
  ND_SHR,       // >>
  ND_EQ,        // ==
  ND_NE,        // !=
  ND_LT,        // <
  ND_LE,        // <=
  ND_ASSIGN,    // =
  ND_COND,      // ?:
  ND_COMMA,     // ,
  ND_MEMBER,    // . (struct member access)
  ND_ADDR,      // unary &
  ND_DEREF,     // unary *
  ND_NOT,       // !
  ND_BITNOT,    // ~
  ND_LOGAND,    // &&
  ND_LOGOR,     // ||
  ND_RETURN,    // "return"
  ND_IF,        // "if"
  ND_FOR,       // "for" or "while"
  ND_DO,        // "do"
  ND_SWITCH,    // "switch"
  ND_CASE,      // "case"
  ND_BLOCK,     // { ... }
  ND_GOTO,      // "goto"
  ND_GOTO_EXPR, // "goto" labels-as-values
  ND_LABEL,     // Labeled statement
  ND_LABEL_VAL, // [GNU] Labels-as-values
  ND_FUNCALL,   // Function call
  ND_EXPR_STMT, // Expression statement
  ND_STMT_EXPR, // Statement expression
  ND_VAR,       // Variable
  ND_VLA_PTR,   // VLA designator
  ND_NUM,       // Integer
  ND_CAST,      // Type cast
  ND_MEMZERO,   // Zero-clear a stack variable
  ND_ASM,       // "asm"
  ND_CAS,       // Atomic compare-and-swap
  ND_EXCH,      // Atomic exchange
  ND_TRAP,      // __builtin_trap and __builtin_unreachable
  ND_CLZ,       // __builtin_clz, __builtin_clzll
  ND_CTZ,       // __builtin_ctz, __builtin_ctzll
  ND_POPCOUNT,  // __builtin_popcount, __builtin_popcountll
  ND_ISNAN,     // __builtin_isnan
  ND_MEMCPY,    // __builtin_memcpy, __builtin_memmove
  ND_MEMSET,    // __builtin_memset
  ND_ATOMIC_CAS,      // __sync_val_compare_and_swap: yields the old value
  ND_ATOMIC_CAS_BOOL, // __sync_bool_compare_and_swap: yields 0 or 1
  ND_ATOMIC_RMW,      // __sync_fetch_and_*, __sync_lock_release
  ND_ATOMIC_WAIT,     // __builtin_wasm_memory_atomic_wait32/64
  ND_ATOMIC_NOTIFY,   // __builtin_wasm_memory_atomic_notify
  ND_ATOMIC_FENCE,    // __sync_synchronize
  ND_MEMORY_SIZE,     // __builtin_wasm_memory_size(0)
  ND_MEMORY_GROW,     // __builtin_wasm_memory_grow(0, n)
  ND_FLOAT_TEST,      // __builtin_isinf, isfinite, isnormal, signbit
  ND_FLOAT_CMP,       // __builtin_isunordered, islessgreater
  ND_OVERFLOW,        // __builtin_add_overflow, sub_overflow, mul_overflow
  ND_TLS,             // __builtin_wasm_tls_base, _size, _align
} NodeKind;

// Which of the three thread-local-storage intrinsics a ND_TLS node is.
enum {
  TLS_BASE = 1,
  TLS_SIZE,
  TLS_ALIGN,
};

// Which test a ND_FLOAT_TEST or ND_FLOAT_CMP node performs. The C99
// classification macros are one family with one shape (floating operands in, int
// out), so they are one node kind each rather than seven.
enum {
  FC_ISINF = 1,
  FC_ISFINITE,
  FC_ISNORMAL,
  FC_SIGNBIT,
  FC_ISUNORDERED,
  FC_ISLESSGREATER,
};

// Which operation a ND_OVERFLOW node performs. It writes its result through a
// pointer and yields 1 when the mathematical result did not fit.
enum {
  OVF_ADD = 1,
  OVF_SUB,
  OVF_MUL,
};

// AST node type
struct Node {
  NodeKind kind; // Node kind
  Node *next;    // Next node
  Type *ty;      // Type, e.g. int or pointer to int
  Token *tok;    // Representative token

  Node *lhs;     // Left-hand side
  Node *rhs;     // Right-hand side

  // "if" or "for" statement
  Node *cond;
  Node *then;
  Node *els;
  Node *init;
  Node *inc;

  // "break" and "continue" labels
  char *brk_label;
  char *cont_label;

  // Block or statement expression
  Node *body;

  // Struct member access
  Member *member;

  // Function call
  Type *func_ty;
  Node *args;
  bool pass_by_stack;
  Obj *ret_buffer;
  // A variadic call marshals its "..." arguments into this local buffer and
  // passes its address as one more argument. NULL when the call passes nothing
  // through "...". See parse.c:funcall and codegen.c:gen_call.
  Obj *va_buf;

  // Goto or labeled statement, or labels-as-values
  char *label;
  char *unique_label;
  Node *goto_next;

  // Switch
  Node *case_next;
  Node *default_case;

  // Case
  long begin;
  long end;

  // "asm" string literal
  char *asm_str;

  // Atomic compare-and-swap
  // ND_ATOMIC_RMW: which read-modify-write ("Add", "Sub", "And", "Or", "Xor",
  // "Xchg", "Store"). The GNU builtins differ only in this name.
  const char *atomic_op;
  Node *cas_addr;
  Node *cas_old;
  Node *cas_new;

  // Atomic op= operators
  Obj *atomic_addr;
  Node *atomic_expr;

  // Variable
  Obj *var;

  // Numeric literal
  int64_t val;
  long double fval;

  // Which test or operation: ND_FLOAT_TEST, ND_FLOAT_CMP and ND_OVERFLOW carry
  // no operator token of their own, so the frontend records the choice here.
  int fclass;
};

Node *new_cast(Node *expr, Type *ty);
int64_t const_expr(Token **rest, Token *tok);
// Parses one translation unit. `tu` numbers it within the program: a file-scope
// static is private to its unit, so the number is part of what a name means.
Obj *parse(Token *tok, int tu);
// Marks the local variables of `fn` whose address escapes. Runs at the end of
// parsing the body, so both the AST dump and code generation see the answer.
void mark_escapes(Obj *fn);

//
// type.c
//

typedef enum {
  TY_VOID,
  TY_BOOL,
  TY_CHAR,
  TY_SHORT,
  TY_INT,
  TY_LONG,
  TY_FLOAT,
  TY_DOUBLE,
  TY_LDOUBLE,
  TY_ENUM,
  TY_PTR,
  TY_FUNC,
  TY_ARRAY,
  TY_VLA, // variable-length array
  TY_STRUCT,
  TY_UNION,
} TypeKind;

struct Type {
  TypeKind kind;
  int size;           // sizeof() value
  int align;          // alignment
  bool is_unsigned;   // unsigned or signed
  bool is_atomic;     // true if _Atomic
  Type *origin;       // for type compatibility check

  // Pointer-to or array-of type. We intentionally use the same member
  // to represent pointer/array duality in C.
  //
  // In many contexts in which a pointer is expected, we examine this
  // member instead of "kind" member to determine whether a type is a
  // pointer or not. That means in many contexts "array of T" is
  // naturally handled as if it were "pointer to T", as required by
  // the C spec.
  Type *base;

  // Declaration
  Token *name;
  Token *name_pos;

  // Array
  int array_len;

  // Variable-length array
  Node *vla_len; // # of elements
  Obj *vla_size; // sizeof() value

  // Struct
  Member *members;
  bool is_flexible;
  bool is_packed;

  // Function type
  Type *return_ty;
  Type *params;
  bool is_variadic;
  Type *next;
};

// Struct member
struct Member {
  Member *next;
  Type *ty;
  Token *tok; // for error message
  Token *name;
  int idx;
  int align;
  int offset;

  // Bitfield
  bool is_bitfield;
  int bit_offset;
  int bit_width;
};

extern Type *ty_void;
extern Type *ty_bool;

extern Type *ty_char;
extern Type *ty_short;
extern Type *ty_int;
extern Type *ty_long;

extern Type *ty_uchar;
extern Type *ty_ushort;
extern Type *ty_uint;
extern Type *ty_ulong;

extern Type *ty_float;
extern Type *ty_double;
extern Type *ty_ldouble;

bool is_integer(Type *ty);
bool is_flonum(Type *ty);
bool is_numeric(Type *ty);
bool is_compatible(Type *t1, Type *t2);
Type *copy_type(Type *ty);
Type *pointer_to(Type *base);
Type *func_type(Type *return_ty);
Type *array_of(Type *base, int size);
Type *vla_of(Type *base, Node *expr);
Type *enum_type(void);
Type *struct_type(void);
void add_type(Node *node);

//
// align.c (njvm-cc: align_to lived in codegen.c, which is not ported)
//

int align_to(int n, int align);

//
// dump.c (njvm-cc: --dump-ast)
//

void dump_ast(Obj *prog, FILE *out);

//
// unicode.c
//

int encode_utf8(char *buf, uint32_t c);
uint32_t decode_utf8(char **new_pos, char *p);
bool is_ident1(uint32_t c);
bool is_ident2(uint32_t c);
int display_width(char *p, int len);

//
// hashmap.c
//

typedef struct {
  char *key;
  int keylen;
  void *val;
} HashEntry;

typedef struct {
  HashEntry *buckets;
  int capacity;
  int used;
} HashMap;

void *hashmap_get(HashMap *map, char *key);
void *hashmap_get2(HashMap *map, char *key, int keylen);
void hashmap_put(HashMap *map, char *key, void *val);
void hashmap_put2(HashMap *map, char *key, int keylen, void *val);
void hashmap_delete(HashMap *map, char *key);
void hashmap_delete2(HashMap *map, char *key, int keylen);
void hashmap_test(void);

//
// main.c
//

bool file_exists(char *path);

extern StringArray include_paths;

// Every file this run of the front end reads, or would have read: the unit
// itself first, then each header in the order the preprocessor reached it.
// A header that an include guard or "#pragma once" skipped is still a
// dependency, because the guard is what decided to skip it; include_file
// records before it looks at either. Two consumers need this list: the
// per-unit cache (a header changing must invalidate the cached unit) and the
// link step (a unit's dependencies are what has to exist before it).
extern StringArray deps;
void record_dep(char *path);
extern bool opt_fpic;
extern bool opt_fcommon;
extern char *base_file;
