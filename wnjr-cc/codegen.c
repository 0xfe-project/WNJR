// wnjr-cc codegen: C AST -> NJVM-IR (MessagePack). Based on chibicc by Rui
// Ueyama (MIT license, see LICENSE).
//
// Shape of the emitted IR
// -----------------------
// Every C object lives in linear memory at a frame-relative address: the
// prologue sets `fp = sp - frame_size` and `sp = fp`, a local at `var->offset`
// is at `fp + offset`, and every Return restores `sp = fp + frame_size`. NJVM
// locals hold only temporaries, so no construct needs a special case for
// having its address taken.
//
// The function body is a flat CFG: a Label per C label, and if/else/loop/switch
// become jumps. That is the shape the IR wants (empty operand stack at labels,
// reducible CFG), and computed goto's markers (Expr::LabelAddr,
// Op::GotoIndirect) are what njvm/computed_goto.rs lowers before the encoder.
//
// Anything this file cannot represent is a hard error naming the construct and
// its source location; nothing is silently miscompiled.

#include "chibicc.h"
#include "njvm.h"

#include <stdlib.h>
#include <string.h>

// Memory layout, lowest address first:
//
//   [GLOBAL_BASE, data_end)          everything with a static extent: globals,
//                                    string literals, static locals, and the
//                                    thread-local block at the front
//   [stack_base, stack_top)          the stack, growing down from stack_top
//   [stack_top, MEMORY_END)          the heap, growing up, and what sbrk hands
//                                    out once this is used up
//
// The stack comes before the heap so that the heap is contiguous with what
// sbrk returns: sbrk grows the memory and hands back the previous end, so a heap
// with a hole above it would break. The stack has no guard page below it, so an
// overflow walks into the data area rather than trapping.
#define MEMORY_PAGES 256
#define MEMORY_END ((uint64_t)MEMORY_PAGES * 65536)
#define GLOBAL_BASE 0x1000
#define STACK_SIZE 0x40000
#define MIN_HEAP 0x10000

// Where the stack starts and the heap ends, both decided by the data layout.
static uint32_t stack_top;

// ───────────────────────── state ─────────────────────────

static Mp *out;

static uint32_t next_label;

// Locals of the function being generated: params first, then temporaries.
static const char **local_ty;
static int nlocals;
static int local_cap;
static int nparams;

// Frame of the function being generated.
static int fp_local;
static int frame_size;
static int sp_global;

// C label name -> IR label id.
typedef struct {
  const char *name;
  uint32_t id;
} LabelEnt;
static LabelEnt *labels;
static int nlabels;
static int label_cap;

// case node -> IR label id.
typedef struct {
  Node *node;
  uint32_t id;
} CaseEnt;
static CaseEnt *cases;
static int ncases;
static int case_cap;

// Imported functions, in source order: wasm puts them first in the function
// index space, so entry k is function index k.
typedef struct {
  const char *module;
  const char *field;
  uint32_t type_idx;
} ImportEnt;
static ImportEnt *imports;
static int nimports;
static int import_cap;

// Ops emitted into the current function body so far.
static size_t body_len;

#define OP(emit)                                                                                   \
  do {                                                                                             \
    emit;                                                                                          \
    body_len++;                                                                                    \
  } while (0)

static noreturn void unsupported(Token *tok, const char *what) {
  // A file-scope object has no token of its own to point at.
  if (!tok)
    error("wnjr-cc: %s is not supported yet", what);
  error_tok(tok, "wnjr-cc: %s is not supported yet", what);
}

// The same, for a message about an object rather than a place in the source:
// a file-scope object has no token, so the message may have to stand alone.
static noreturn void error_obj(Obj *var, const char *msg) {
  if (!var->tok)
    error("wnjr-cc: %s", msg);
  error_tok(var->tok, "wnjr-cc: %s", msg);
}

static void gen_stmt(Node *node);
static E *gen_expr(Node *node);

// ───────────────────────── types ─────────────────────────

static bool is_aggregate(Type *ty) {
  return ty->kind == TY_STRUCT || ty->kind == TY_UNION || ty->kind == TY_ARRAY ||
         ty->kind == TY_VLA;
}

static bool is_scalar(Type *ty) {
  return !is_aggregate(ty) && ty->kind != TY_VOID;
}

// A C value that *is* its address: an array or a struct decays to it, and a
// function designator becomes a pointer to the function. Reading one of these
// would load the wrong thing, so an expression of such a type yields the
// address.
static bool is_addr_value(Type *ty) {
  return is_aggregate(ty) || ty->kind == TY_FUNC;
}

// Pointers occupy eight bytes in chibicc's LP64 layout (type.c), but only the
// low 32 bits are an address today, so a pointer is read and written as an i32
// word and the upper half is left alone. #634 puts the GC tag in that upper
// half; when it lands this becomes "I64" and nothing else in this file moves.
#define PTR_MEM "I32"

// Where the thread-local block is. There is one execution context in this
// runtime, so there is one TLS block, laid out with the globals and never copied:
// the three __builtin_wasm_tls_* intrinsics report these numbers, and a runtime
// with threads would use them to give each thread its own copy of the initial
// data. Filled in by assign_global_offsets.
static int tls_base;
static int tls_size;
static int tls_align = 1;

// The NJVM-IR type of a C scalar. A pointer is an i32: a wasm32 address.
//
// The signedness of a 4- or 8-byte integer is part of the type, because the IR
// picks the signed or the unsigned instruction from it: an unsigned divide, a
// right shift, a less-than. Narrow integers are promoted to int before any
// arithmetic (usual_arith_conv), so they never reach here needing a U type.
static const char *ir_ty(Type *ty) {
  switch (ty->kind) {
  case TY_FLOAT:
    return "F32";
  case TY_DOUBLE:
  case TY_LDOUBLE:
    return "F64";
  case TY_PTR:
  case TY_ARRAY:
  case TY_FUNC:
    return PTR_MEM;
  case TY_STRUCT:
  case TY_UNION:
  case TY_VLA:
    error("internal error: %s is not a scalar type", __FILE__);
  default:
    if (ty->size == 8)
      return ty->is_unsigned ? "U64" : "I64";
    return ty->is_unsigned ? "U32" : "I32";
  }
}

// The memory access width of a C scalar.
static const char *mem_ty(Type *ty) {
  switch (ty->kind) {
  case TY_FLOAT:
    return "F32";
  case TY_DOUBLE:
  case TY_LDOUBLE:
    return "F64";
  case TY_PTR:
  case TY_ARRAY:
  case TY_FUNC:
    // NJVM-IR has no pointer MemType; an address is an i32 word.
    return PTR_MEM;
  default:
    break;
  }
  switch (ty->size) {
  case 1:
    return ty->is_unsigned ? "U8" : "I8";
  case 2:
    return ty->is_unsigned ? "U16" : "I16";
  case 4:
    return "I32";
  case 8:
    return "I64";
  default:
    error("internal error: bad scalar size %d", ty->size);
  }
}

// The IR type a Load of `mem_ty(ty)` produces is the MemType itself (see
// load_type in verify.rs), so a narrow load is converted before it is used as
// an int.

// ─────────────────── function-pointer signatures ───────────────────

// One C function type reached through a pointer. It becomes one entry in the
// module's `types` list and an indirect call names it by that index. Only
// scalars cross `call_indirect`: a struct by value would need a different
// convention, and nothing here supports one yet.
typedef struct {
  const char **params;
  int nparams;
  const char *result; // NULL for a void return
} Sig;

static Sig *sigs;
static int nsigs;
static int sig_cap;
// Set when a function address is taken or a call goes through one: the module
// then needs the function-pointer table and the element segment that fills it.
static bool indirect_used;

// A program is every translation unit spliced into one object list, and a name
// may be written in one unit and defined in another. Every question about an
// object's address or index goes through this table.
//
// Two objects share a name when one is the definition and the others declare it,
// or when each is a file-scope static of a different unit, in which case the name
// means the one belonging to the unit asking.
static HashMap *syms;

// Symbols the program refers to and nothing defines. Compilation carries on past
// them -- a missing function gets index 0 and a missing object address 0 -- so
// that one run reports all of them instead of the first. The module is never
// written when the list is not empty.
static StringArray missing;
static StringArray missing_where;

static void note_missing(Token *tok, const char *name) {
  for (int i = 0; i < missing.len; i++)
    if (!strcmp(missing.data[i], name))
      return;
  strarray_push(&missing, (char *)name);
  strarray_push(&missing_where,
                tok ? format("%s:%d", tok->file->name, tok->line_no)
                    : (char *)"a static initialiser");
}

// The definition a name has in the program, or NULL. Only a definition, an
// import or an alias answers: a bare declaration is not something to call or to
// take the address of.
static Obj *find_symbol(const char *name, int tu) {
  if (!syms || !name)
    return NULL;
  Obj *static_hit = NULL;
  for (Obj *o = hashmap_get(syms, (char *)name); o; o = o->name_next) {
    if (o->is_static && o->tu != tu)
      continue;
    if (!o->is_definition && !o->import_module && !o->is_alias)
      continue;
    // The definition wins over a static of the same name in another unit.
    if (!o->is_static)
      return o;
    if (!static_hit)
      static_hit = o;
  }
  return static_hit;
}

// The object behind a reference. A reference is written against whatever
// declaration was in scope, which in a program of several units is usually not
// the definition.
static Obj *definition_of(Obj *ref) {
  if (ref->is_definition || ref->import_module || ref->is_alias)
    return ref;
  Obj *def = find_symbol(ref->name, ref->tu);
  return def ? def : ref;
}

// C says a tentative definition -- `int x;` at file scope -- gives way to a real
// one, and that two tentative definitions are one object. parse.c collapses them
// inside a unit; this does it across units, and reports a name that is really
// defined twice. A dropped object stops being a definition, so a reference that
// points at it resolves through the table to the one that stayed.
static Obj *collapse_definitions(Obj *prog) {
  HashMap *canon = calloc(1, sizeof(HashMap));
  for (Obj *var = prog; var; var = var->next) {
    if (!var->name || !var->is_definition || var->is_static || var->is_alias)
      continue;
    Obj *prev = hashmap_get(canon, var->name);
    if (!prev) {
      hashmap_put(canon, var->name, var);
      continue;
    }
    if (!prev->is_tentative && !var->is_tentative)
      error_obj(var,
                format("%s is defined twice: here and in %s", var->name,
                       prev->tok ? prev->tok->file->name : "another unit"));
    // The real definition replaces a tentative one; two tentative ones keep the
    // first, whose address the program may already have written down.
    if (!var->is_tentative && prev->is_tentative) {
      prev->is_definition = false;
      hashmap_put(canon, var->name, var);
    } else if (var->is_tentative && !prev->is_tentative) {
      var->is_definition = false;
    } else {
      var->is_definition = false;
    }
  }

  Obj head;
  Obj *cur = &head;
  for (Obj *var = prog; var;) {
    Obj *next = var->next;
    if (var->name && var->is_definition == false && var->is_tentative) {
      var = next;
      continue;
    }
    cur = cur->next = var;
    var = next;
  }
  cur->next = NULL;
  return head.next;
}

static void build_symbols(Obj *prog) {
  syms = calloc(1, sizeof(HashMap));
  for (Obj *var = prog; var; var = var->next) {
    if (!var->name)
      continue;
    var->name_next = hashmap_get(syms, var->name);
    hashmap_put(syms, var->name, var);
  }
}


// chibicc marks a function whose parameter list is empty -- f() -- variadic,
// because at that point in the parser an empty list and "..." are the same thing.
// Here they are not: C requires a named parameter before the "...", so a variadic
// type with no parameters is an old-style declaration with none, and the calling
// convention treats it as taking nothing extra.
static bool takes_varargs(Type *fnty) {
  return fnty && fnty->is_variadic && fnty->params != NULL;
}

// The wasm type of parameter j of a C function type, under this frontend's two
// conventions: a struct or union return is one hidden address first, and a
// variadic function takes the caller's argument buffer last.
static const char *param_ir_type(Type *fnty, bool sret, int j) {
  int idx = sret ? 1 : 0;
  for (Type *p = fnty->params; p; p = p->next, idx++)
    if (idx == j)
      return is_aggregate(p) ? PTR_MEM : ir_ty(p);
  return PTR_MEM;
}

// The index in `Module::types` of this function type, adding it if new.
static uint32_t sig_of(Token *tok, Type *fnty) {
  if (!fnty || fnty->kind != TY_FUNC)
    unsupported(tok, "a call through something that is not a function pointer");

  // A struct parameter travels as the address of the caller's copy and a
  // struct return goes through a hidden first parameter, so each is one i32
  // in the wasm signature — the same convention a direct call uses.
  bool sret = is_aggregate(fnty->return_ty);
  int n = sret ? 1 : 0;
  for (Type *p = fnty->params; p; p = p->next) {
    if (!is_scalar(p) && !is_aggregate(p))
      unsupported(tok, "a parameter of this type through a function pointer");
    n++;
  }
  // A variadic callee takes the argument buffer's address as one more
  // parameter, so that its signature matches what gen_call passes.
  if (takes_varargs(fnty))
    n++;
  const char *result = NULL;
  if (!sret && fnty->return_ty->kind != TY_VOID) {
    if (!is_scalar(fnty->return_ty))
      unsupported(tok, "returning this type by value through a function pointer");
    result = ir_ty(fnty->return_ty);
  }

  for (int k = 0; k < nsigs; k++) {
    Sig *s = &sigs[k];
    if (s->nparams != n || (s->result == NULL) != (result == NULL))
      continue;
    if (result && strcmp(result, s->result))
      continue;
    bool same = true;
    for (int j = 0; j < n && same; j++)
      if (strcmp(s->params[j], param_ir_type(fnty, sret, j)))
        same = false;
    if (same)
      return (uint32_t)k;
  }

  if (nsigs == sig_cap) {
    sig_cap = sig_cap ? sig_cap * 2 : 8;
    sigs = realloc(sigs, (size_t)sig_cap * sizeof *sigs);
    if (!sigs)
      error("out of memory");
  }
  Sig *s = &sigs[nsigs];
  s->params = n ? calloc((size_t)n, sizeof *s->params) : NULL;
  if (n && !s->params)
    error("out of memory");
  s->nparams = n;
  s->result = result;
  int i = 0;
  if (sret)
    s->params[i++] = PTR_MEM;
  for (Type *p = fnty->params; p; p = p->next)
    s->params[i++] = is_aggregate(p) ? PTR_MEM : ir_ty(p);
  if (takes_varargs(fnty))
    s->params[i++] = PTR_MEM;
  return (uint32_t)nsigs++;
}

// ───────────────────────── locals ─────────────────────────

static void reset_locals(void) {
  nlocals = 0;
  nparams = 0;
}

static int add_local(const char *ty) {
  if (nlocals == local_cap) {
    local_cap = local_cap ? local_cap * 2 : 64;
    local_ty = realloc(local_ty, local_cap * sizeof(*local_ty));
    if (!local_ty)
      error("out of memory");
  }
  local_ty[nlocals] = ty;
  return nlocals++;
}

static int new_temp(const char *ty) {
  return add_local(ty);
}

static E *e_addr(E *base, int offset) {
  if (!offset)
    return base;
  return e_binary("Add", "I32", base, e_i32(offset));
}

static E *frame_addr(int offset) {
  return e_addr(e_local(fp_local, "I32"), offset);
}

// ───────────────────────── labels ─────────────────────────

static void reset_labels(void) {
  next_label = 1;
  nlabels = 0;
  ncases = 0;
}

static uint32_t label_id(const char *name) {
  for (int i = 0; i < nlabels; i++)
    if (!strcmp(labels[i].name, name))
      return labels[i].id;
  if (nlabels == label_cap) {
    label_cap = label_cap ? label_cap * 2 : 32;
    labels = realloc(labels, label_cap * sizeof(*labels));
    if (!labels)
      error("out of memory");
  }
  labels[nlabels].name = name;
  labels[nlabels].id = next_label++;
  return labels[nlabels++].id;
}

static uint32_t fresh_label(void) {
  return next_label++;
}

static void add_case(Node *node, uint32_t id) {
  if (ncases == case_cap) {
    case_cap = case_cap ? case_cap * 2 : 32;
    cases = realloc(cases, case_cap * sizeof(*cases));
    if (!cases)
      error("out of memory");
  }
  cases[ncases].node = node;
  cases[ncases].id = id;
  ncases++;
}

static uint32_t case_id(Node *node) {
  for (int i = 0; i < ncases; i++)
    if (cases[i].node == node)
      return cases[i].id;
  error_tok(node->tok, "internal error: case label without an id");
}

// ───────────────────────── expressions ─────────────────────────

// Materialize an expression into a local so it can be read more than once or
// evaluated before something with a side effect runs.
static E *to_temp(E *e) {
  if (e->kind == E_LOCAL)
    return e;
  int l = new_temp(e->ty);
  OP(nj_op_localset(out, l, e));
  e_free(e);
  return e_local(l, local_ty[l]);
}

static E *gen_addr(Node *node) {
  switch (node->kind) {
  case ND_VAR:
    if (node->var->is_local)
      return frame_addr(node->var->offset);
    if (node->var->is_function) {
      // A C function pointer is an index into table 0, and the element
      // segment puts function i at index i, so the function's own index is
      // the value to use. An imported function has an index too, and one
      // defined in another translation unit is found by name.
      Obj *fn = definition_of(node->var);
      indirect_used = true;
      if (!fn->is_definition && !fn->import_module && !fn->is_alias) {
        note_missing(node->tok, fn->name);
        return e_i32(0);
      }
      return e_i32(fn->offset);
    }
    Obj *def = definition_of(node->var);
    if (!def->is_definition) {
      note_missing(node->tok, def->name);
      return e_i32(0);
    }
    return e_i32(def->offset);
  case ND_DEREF:
    return gen_expr(node->lhs);
  case ND_MEMBER: {
    E *base = gen_addr(node->lhs);
    return e_addr(base, node->member->offset);
  }
  // An expression that *is* an aggregate has its address as its value, so an
  // aggregate-valued call or assignment can be indexed, member-selected or
  // copied like any other object.
  case ND_FUNCALL:
  case ND_ASSIGN:
  case ND_COMMA:
  case ND_COND:
  case ND_STMT_EXPR:
    if (is_aggregate(node->ty))
      return to_temp(gen_expr(node));
    unsupported(node->tok, "taking the address of this expression");
  default:
    unsupported(node->tok, "taking the address of this expression");
  }
}

static E *gen_load(Node *node, Type *ty, E *addr) {
  if (!is_scalar(ty))
    unsupported(node->tok, "reading a struct, union or array by value");
  const char *mt = mem_ty(ty);
  const char *want = ir_ty(ty);
  E *v = e_load(mt, addr, 0, 0);
  if (strcmp(mt, want))
    return e_convert(mt, want, v);
  return v;
}

// A store takes the type its memory type produces on a load (see gen_load), so a
// value of another IR type of the same width is converted first. An unsigned
// 32-bit value into an i32 word is the pair that occurs: same bits, and the
// conversion only says which of the two they are.
static E *store_value(E *v, const char *mt) {
  if (!strcmp(v->ty, mt))
    return v;
  return e_convert(v->ty, mt, v);
}

static const char *binary_op_name(NodeKind kind) {
  switch (kind) {
  case ND_ADD:
    return "Add";
  case ND_SUB:
    return "Sub";
  case ND_MUL:
    return "Mul";
  case ND_DIV:
    return "Div";
  case ND_MOD:
    return "Rem";
  case ND_BITAND:
    return "And";
  case ND_BITOR:
    return "Or";
  case ND_BITXOR:
    return "Xor";
  case ND_SHL:
    return "Shl";
  case ND_SHR:
    return "Shr";
  case ND_EQ:
    return "Eq";
  case ND_NE:
    return "Ne";
  case ND_LT:
    return "Lt";
  case ND_LE:
    return "Le";
  default:
    error("internal error: %d is not a binary operator", kind);
  }
}

// Convert an expression to `want` unless it already has that type.
static E *as_type(E *e, const char *want) {
  if (!strcmp(e->ty, want))
    return e;
  return e_convert(e->ty, want, e);
}

// The type two operands are brought to. Their generated types decide, which
// keeps the emitted op and its operands consistent by construction.
//
// Two operands that already agree keep their type exactly: U32 and I32 are the
// same width but not the same operation, so collapsing an unsigned operand into
// a signed one here would turn an unsigned divide into a signed one.
static const char *common_ty(E *a, E *b) {
  if (!strcmp(a->ty, b->ty))
    return a->ty;
  if (!strcmp(a->ty, "F64") || !strcmp(b->ty, "F64"))
    return "F64";
  if (!strcmp(a->ty, "F32") || !strcmp(b->ty, "F32"))
    return "F32";
  if (!strcmp(a->ty, "I64") || !strcmp(b->ty, "I64") ||
      !strcmp(a->ty, "U64") || !strcmp(b->ty, "U64"))
    return "I64";
  return "I32";
}

static bool is_comparison(NodeKind kind) {
  return kind == ND_EQ || kind == ND_NE || kind == ND_LT || kind == ND_LE;
}

// A comparison takes operands of one type and yields an i32, so its result type
// and its operand type are different; e_binary() sets both from one type, which
// is right for arithmetic and wrong here.
static E *cmp_(const char *op, const char *oty, E *a, E *b) {
  E *r = e_new(E_BINARY, "I32");
  r->op = op;
  r->op_ty = oty;
  r->a = a;
  r->b = b;
  return r;
}

static E *and_(E *a, E *b) { return e_binary("And", "I32", a, b); }
static E *or_(E *a, E *b) { return e_binary("Or", "I32", a, b); }

// cond ? then : else, as control flow rather than a select. wasm evaluates both
// arms of a select, and the arm this guards is a division that traps on exactly
// the value the condition excludes.
static E *guarded(const char *ty, E *cond, E *then_val, E *else_val) {
  int t = new_temp(ty);
  uint32_t l_else = fresh_label();
  uint32_t l_end = fresh_label();
  OP(nj_op_jmpif(out, e_binary("Eq", "I32", cond, e_i32(0)), l_else));
  OP(nj_op_localset(out, (uint32_t)t, then_val));
  OP(nj_op_jmp(out, l_end));
  OP(nj_op_label(out, l_else));
  OP(nj_op_localset(out, (uint32_t)t, else_val));
  OP(nj_op_label(out, l_end));
  return e_local((uint32_t)t, ty);
}

static E *gen_binary(Node *node) {
  E *a = gen_expr(node->lhs);
  E *b = gen_expr(node->rhs);
  // C converts only the left operand of a shift; the count keeps its own type.
  // Taking the common type here would let an int literal's signedness decide the
  // instruction, which turns an unsigned right shift into an arithmetic one.
  const char *ty;
  if (node->kind == ND_SHL || node->kind == ND_SHR) {
    ty = a->ty;
    b = as_type(b, ty);
  } else {
    ty = common_ty(a, b);
    a = as_type(a, ty);
    b = as_type(b, ty);
  }
  E *e = e_new(E_BINARY, is_comparison(node->kind) ? "I32" : ty);
  e->op = binary_op_name(node->kind);
  e->op_ty = ty;
  e->a = a;
  e->b = b;
  return e;
}

static E *gen_logical(Node *node) {
  // 0/1, stopping at the first decisive operand.
  int t = new_temp("I32");
  uint32_t l_end = fresh_label();
  E *a = gen_expr(node->lhs);
  OP(nj_op_localset(out, t, e_binary("Ne", "I32", a, e_i32(0))));
  if (node->kind == ND_LOGAND)
    OP(nj_op_jmpif(out, e_binary("Eq", "I32", e_local(t, "I32"), e_i32(0)), l_end));
  else
    OP(nj_op_jmpif(out, e_local(t, "I32"), l_end));
  E *b = gen_expr(node->rhs);
  OP(nj_op_localset(out, t, e_binary("Ne", "I32", b, e_i32(0))));
  OP(nj_op_label(out, l_end));
  return e_local(t, "I32");
}

// The access width of an atomic on the object `ptr` points at, in bytes. wasm's
// atomics exist for 1, 2, 4 and 8 bytes and nothing else.
static int atomic_width(Node *ptr) {
  if (ptr->ty->kind != TY_PTR)
    unsupported(ptr->tok, "an atomic needs a pointer");
  Type *base = ptr->ty->base;
  if (!base)
    unsupported(ptr->tok, "an atomic on a pointer to nothing");
  int size = base->size;
  if (size == 1 || size == 2 || size == 4 || size == 8)
    return size;
  if (size == 0)
    unsupported(ptr->tok, "an atomic on a zero-sized value");
  unsupported(ptr->tok, "an atomic on a value of a width wasm has no atomic for");
  return 0;
}

// wasm's narrow atomics exist only in their unsigned form (i32.atomic.rmw16.add_u),
// so a load or a read-modify-write of a signed 1- or 2-byte object comes back
// zero-extended. The C value is the sign-extended one, and the two-step convert
// is how the IR says so: to the narrow signed type, which normalizes, then to
// the i32 a local holds.
static E *narrow_atomic_result(E *v, Type *cty) {
  if (cty->size != 1 && cty->size != 2)
    return v;
  if (cty->is_unsigned)
    return v;
  const char *narrow = cty->size == 1 ? "I8" : "I16";
  // Narrow to the signed type (the normalize step), then back out to the i32 a
  // local can hold.
  return e_convert(narrow, ir_ty(cty), e_convert("I32", narrow, v));
}

// The IR type a value of C type `ty` is carried in. Narrow integers travel as
// i32, so this is the type a temp for that C type should have.
static const char *value_ty(Type *ty) {
  return ir_ty(ty);
}

static E *gen_probe_ty(Node *node) {
  // A zero of the node's value type, discarded: only its type is wanted.
  E *e = e_new(E_CONST_I32, value_ty(node->ty));
  return e;
}

static E *gen_cond(Node *node) {
  const char *ty = common_ty(gen_probe_ty(node->then), gen_probe_ty(node->els));
  int t = new_temp(ty);
  uint32_t l_else = fresh_label();
  uint32_t l_end = fresh_label();
  E *c = gen_expr(node->cond);
  OP(nj_op_jmpif(out, e_binary("Eq", "I32", c, e_i32(0)), l_else));
  E *a = as_type(gen_expr(node->then), ty);
  OP(nj_op_localset(out, t, a));
  OP(nj_op_jmp(out, l_end));
  OP(nj_op_label(out, l_else));
  E *b = as_type(gen_expr(node->els), ty);
  OP(nj_op_localset(out, t, b));
  OP(nj_op_label(out, l_end));
  return e_local(t, ty);
}

static uint32_t func_index(Obj *fn) {
  if (!fn->is_function)
    error_tok(fn->tok, "wnjr-cc: not a function");
  // A declared but undefined function is callable only when an import attribute
  // said where it comes from, when it is another name for a function here, or
  // when another unit defines it.
  Obj *def = definition_of(fn);
  if (!def->is_definition && !def->import_module && !def->is_alias) {
    note_missing(fn->tok, fn->name);
    return 0;
  }
  return (uint32_t)def->offset;
}

// One "..." argument, stored into the caller's buffer at `off`. The callee reads
// it back with va_arg, which walks the same slots in the same order: a slot is
// rounded to eight bytes, and a wider alignment pads before it.
static void store_va_arg(Obj *vb, int off, Type *ty, E *val) {
  E *addr = e_addr(frame_addr(vb->offset), off);
  // An address — a pointer, or an array that decayed to one — is 32 bits wide in
  // a 64-bit slot, and va_arg reads the whole slot as a pointer, so the upper
  // half has to be a defined zero.
  if (!strcmp(ir_ty(ty), PTR_MEM)) {
    OP(nj_op_store(out, "I64", addr, e_convert(PTR_MEM, "I64", val), 0, 0));
    return;
  }
  OP(nj_op_store(out, mem_ty(ty), addr, store_value(val, mem_ty(ty)), 0, 0));
}

static E *gen_call(Node *node) {
  Node *callee = node->lhs;
  // A direct call names a function of this translation unit; anything else is
  // an indirect call through a function pointer, whose value is a table index.
  bool direct = callee->kind == ND_VAR && callee->var->is_function;
  uint32_t idx = 0;
  uint32_t type_idx = 0;
  if (direct) {
    idx = func_index(callee->var);
  } else {
    Type *fnty = callee->ty->kind == TY_FUNC ? callee->ty : callee->ty->base;
    type_idx = sig_of(node->tok, fnty);
    indirect_used = true;
  }

  // A struct return goes through a buffer the caller owns; parse.c has already
  // made it a local of this function, so it has a frame slot before the body is
  // generated, and its address is the hidden first argument.
  Obj *rb = is_aggregate(node->ty) ? node->ret_buffer : NULL;
  if (is_aggregate(node->ty) && !rb)
    unsupported(node->tok, "a struct or union returned by this call");

  int nargs = 0;
  for (Node *arg = node->args; arg; arg = arg->next)
    nargs++;
  E **args = calloc((size_t)nargs + 1, sizeof(E *));
  if (!args)
    error("out of memory");
  // A variadic callee's "..." arguments are not parameters: the caller lays them
  // out in a buffer of its own frame and passes that buffer's address as the
  // last argument (parse.c:funcall made the buffer a local).
  bool variadic = takes_varargs(node->func_ty);
  Obj *vb = variadic ? node->va_buf : NULL;
  Type *declared = node->func_ty ? node->func_ty->params : NULL;
  int slot = 0;

  int i = 0;
  if (rb)
    args[i++] = e_i32(rb->offset);
  for (Node *arg = node->args; arg; arg = arg->next) {
    // A struct argument is passed as the address of the argument object; the
    // callee copies it into its own frame, so by-value semantics hold without a
    // temporary here. parse.c inserts no cast for a struct parameter, so the
    // value of the expression is already that address.
    if (!is_scalar(arg->ty) && !is_aggregate(arg->ty))
      unsupported(arg->tok, "a variable-length array argument");
    E *val = gen_expr(arg);
    if (declared) {
      args[i++] = val;
      declared = declared->next;
      continue;
    }
    // Everything from here on goes through "...".
    if (!vb)
      unsupported(arg->tok, "an argument with no parameter to travel in");
    int align = arg->ty->align < 8 ? 8 : arg->ty->align;
    slot = align_to(slot, align);
    store_va_arg(vb, slot, arg->ty, val);
    slot += align_to(arg->ty->size, 8);
    e_free(val);
  }
  if (variadic)
    args[i++] = vb ? frame_addr(vb->offset) : e_i32(0);
  size_t narg = (size_t)i;

  // The arguments are evaluated before the callee (erase.rs documents that
  // order for CallIndirect), so a callee expression that itself contains a
  // call cannot disturb the argument temporaries.
  E *callee_e = direct ? NULL : to_temp(gen_expr(callee));

  E *result;
  if (rb) {
    // The callee wrote into the buffer and returned nothing; the value of the
    // call is that buffer's address.
    if (direct)
      OP(nj_op_call(out, idx, args, narg));
    else
      OP(nj_op_callindirect(out, type_idx, 0, callee_e, args, narg));
    result = e_i32(rb->offset);
  } else if (node->ty->kind == TY_VOID) {
    if (direct)
      OP(nj_op_call(out, idx, args, narg));
    else
      OP(nj_op_callindirect(out, type_idx, 0, callee_e, args, narg));
    result = e_i32(0);
  } else {
    int t = new_temp(ir_ty(node->ty));
    if (direct)
      OP(nj_op_localset_call(out, (uint32_t)t, idx, args, narg));
    else
      OP(nj_op_localset_callindirect(out, (uint32_t)t, type_idx, 0, callee_e, args, narg));
    result = e_local(t, local_ty[t]);
  }
  for (size_t j = 0; j < narg; j++)
    e_free(args[j]);
  free(args);
  if (callee_e)
    e_free(callee_e);
  return result;
}

static E *gen_expr(Node *node) {
  if (!node)
    return e_i32(0);

  switch (node->kind) {
  case ND_NULL_EXPR:
    return e_i32(0);

  // A declaration with an initializer zeroes the object first; that reaches
  // codegen as an expression inside a comma.
  case ND_MEMZERO:
    OP(nj_op_memoryfill(out, frame_addr(node->var->offset), e_i32(0), e_i32(node->var->ty->size)));
    return e_i32(0);

  case ND_NUM:
    switch (node->ty->kind) {
    case TY_FLOAT: {
      // The bit pattern, not the value rounded to an integer.
      float v = (float)node->fval;
      uint32_t bits;
      memcpy(&bits, &v, sizeof bits);
      return e_f32(bits);
    }
    case TY_DOUBLE:
    case TY_LDOUBLE: {
      double v = (double)node->fval;
      uint64_t bits;
      memcpy(&bits, &v, sizeof bits);
      return e_f64(bits);
    }
    default:
      // A constant carries its C type's signedness into the IR, because that is
      // what decides which instruction consumes it: a size_t constant is a U64,
      // and treating it as an I64 makes a fill's length the wrong type.
      if (node->ty->size == 8)
        return as_type(e_i64(node->val), ir_ty(node->ty));
      return as_type(e_i32((int32_t)node->val), ir_ty(node->ty));
    }

  case ND_NEG:
    return e_unary("Neg", ir_ty(node->ty), gen_expr(node->lhs));

  case ND_BITNOT:
    return e_unary("Not", ir_ty(node->ty), gen_expr(node->lhs));

  // A trap is an unreachable instruction: the engine stops here with no value,
  // which is what both spellings mean.
  case ND_TRAP:
    OP(nj_op_unreachable(out));
    return e_i32(0);

  // The IR types a Unary by its operand type, so a 64-bit operand names an I64
  // unary; the C result is an int either way, so it is converted back.
  case ND_CLZ:
  case ND_CTZ:
  case ND_POPCOUNT: {
    const char *oty = ir_ty(node->lhs->ty);
    const char *op = node->kind == ND_CLZ     ? "Clz"
                     : node->kind == ND_CTZ   ? "Ctz"
                                              : "Popcnt";
    E *val = gen_expr(node->lhs);
    E *r = e_unary(op, oty, val);
    if (strcmp(oty, "I32"))
      return e_convert(oty, "I32", r);
    return r;
  }

  // x != x is true only for a NaN, and the operand is read once.
  case ND_ISNAN: {
    const char *oty = ir_ty(node->lhs->ty);
    int t = new_temp(oty);
    OP(nj_op_localset(out, (uint32_t)t, gen_expr(node->lhs)));
    // A comparison takes operands of one type and yields an i32, which is why
    // the result type and op_ty are set separately here (as gen_binary does).
    E *r = e_new(E_BINARY, "I32");
    r->op = "Ne";
    r->op_ty = oty;
    r->a = e_local((uint32_t)t, oty);
    r->b = e_local((uint32_t)t, oty);
    return r;
  }

  // The C99 classification family, as comparisons the IR already has: no new
  // instruction is needed for any of them. The operand is read once into a temp
  // local, and every use of it is a separate read of that local: e_free() walks
  // a tree and frees every node in it, so one node reached twice would be freed
  // twice.
  case ND_TLS: {
    // The block is laid out with the globals, so all three are known here.
    int64_t v = node->fclass == TLS_BASE   ? tls_base
                : node->fclass == TLS_SIZE ? tls_size
                                           : tls_align;
    if (node->ty->size == 8)
      return as_type(e_i64(v), ir_ty(node->ty));
    return as_type(e_i32((int32_t)v), ir_ty(node->ty));
  }

  case ND_FLOAT_TEST: {
    const char *oty = ir_ty(node->lhs->ty);
    bool f32 = !strcmp(oty, "F32");
    int t = new_temp(oty);
    OP(nj_op_localset(out, (uint32_t)t, gen_expr(node->lhs)));
#define V e_local((uint32_t)t, oty)
    // Constants are made per use, like any other node: the tree is freed by
    // ownership, so one node in two places is one node freed twice.
#define ZERO (f32 ? e_f32(0) : e_f64(0))
#define ONE (f32 ? e_f32(0x3f800000u) : e_f64(0x3ff0000000000000ull))
#define INF (f32 ? e_f32(0x7f800000u) : e_f64(0x7ff0000000000000ull))
#define NINF (f32 ? e_f32(0xff800000u) : e_f64(0xfff0000000000000ull))
#define FMIN (f32 ? e_f32(0x00800000u) : e_f64(0x0010000000000000ull))
#define NFMIN (f32 ? e_f32(0x80800000u) : e_f64(0x8010000000000000ull))

    switch (node->fclass) {
    case FC_ISINF:
      return or_(cmp_("Eq", oty, V, INF), cmp_("Eq", oty, V, NINF));
    case FC_ISFINITE:
      // x - x is zero exactly when x is neither infinite nor a NaN.
      return cmp_("Eq", oty, e_binary("Sub", oty, V, V), ZERO);
    case FC_ISNORMAL:
      // Not zero and not subnormal: |x| is at least the smallest normal number.
      // The finite half of the definition is the same subtraction as above.
      return and_(or_(cmp_("Ge", oty, V, FMIN), cmp_("Le", oty, V, NFMIN)),
                  cmp_("Eq", oty, e_binary("Sub", oty, V, V), ZERO));
    case FC_SIGNBIT:
      // The sign bit is not readable from the value, but only a negative number
      // satisfies either x < 0 or (x is a zero and one over it is negative). The
      // first clause is what makes -inf work and the second what makes -0 work.
      return or_(cmp_("Lt", oty, V, ZERO),
                 and_(cmp_("Eq", oty, V, ZERO),
                      cmp_("Lt", oty, e_binary("Div", oty, ONE, V), ZERO)));
    default:
      error("internal error: unknown floating-point test %d", node->fclass);
      return e_i32(0);
    }
#undef V
#undef ZERO
#undef ONE
#undef INF
#undef NINF
#undef FMIN
#undef NFMIN
  }

  case ND_FLOAT_CMP: {
    const char *oty = ir_ty(node->lhs->ty);
    int a = new_temp(oty), b = new_temp(oty);
    OP(nj_op_localset(out, (uint32_t)a, gen_expr(node->lhs)));
    OP(nj_op_localset(out, (uint32_t)b, gen_expr(node->rhs)));
#define X e_local((uint32_t)a, oty)
#define Y e_local((uint32_t)b, oty)
    if (node->fclass == FC_ISUNORDERED)
      return or_(cmp_("Ne", oty, X, X), cmp_("Ne", oty, Y, Y));
    // islessgreater: both comparisons are false when either operand is a NaN,
    // which is the C answer for an unordered pair.
    return or_(cmp_("Lt", oty, X, Y), cmp_("Gt", oty, X, Y));
#undef X
#undef Y
  }

  // __builtin_{add,sub,mul}_overflow writes the wrapped result and answers
  // whether it wrapped. Each operand is read once into a temp, and a check that
  // needs a division is guarded, because a division traps on the value its own
  // condition excludes.
  case ND_OVERFLOW: {
    Type *ty = node->lhs->ty;
    const char *ity = ir_ty(ty);
    bool sgn = !ty->is_unsigned;
    bool wide = ty->size == 8;
    E *addr = to_temp(gen_expr(node->cas_addr));
    int a = new_temp(ity), b = new_temp(ity);
    OP(nj_op_localset(out, (uint32_t)a, as_type(gen_expr(node->lhs), ity)));
    OP(nj_op_localset(out, (uint32_t)b, as_type(gen_expr(node->rhs), ity)));
    const char *op = node->fclass == OVF_ADD   ? "Add"
                     : node->fclass == OVF_SUB ? "Sub"
                                               : "Mul";
#define A e_local((uint32_t)a, ity)
#define B e_local((uint32_t)b, ity)
#define RES e_binary(op, ity, A, B)
#define IZERO as_type(wide ? e_i64(0) : e_i32(0), ity)
    OP(nj_op_store(out, mem_ty(ty), addr, store_value(RES, mem_ty(ty)), 0, 0));
    e_free(addr);

    if (!sgn && node->fclass == OVF_ADD)
      return cmp_("Lt", ity, RES, A); // a sum that wrapped is below an operand
    if (!sgn && node->fclass == OVF_SUB)
      return cmp_("Lt", ity, A, B);
    if (!sgn)
      // (a * b) / a == b holds exactly when the multiplication did not wrap, and
      // a == 0 wraps nothing (and is the division that would trap).
      return guarded("I32", cmp_("Eq", ity, A, IZERO), e_i32(0),
                     cmp_("Ne", ity, e_binary("Div", ity, RES, A), B));
    // Signed: the result's sign differs from what the operation would give.
    if (node->fclass == OVF_ADD)
      return or_(and_(cmp_("Gt", ity, B, IZERO), cmp_("Lt", ity, RES, A)),
                 and_(cmp_("Lt", ity, B, IZERO), cmp_("Gt", ity, RES, A)));
    if (node->fclass == OVF_SUB)
      return or_(and_(cmp_("Gt", ity, B, IZERO), cmp_("Gt", ity, RES, A)),
                 and_(cmp_("Lt", ity, B, IZERO), cmp_("Lt", ity, RES, A)));
    // Signed mul: the same division test, except that a == -1 with b == the most
    // negative value is a wrap the division cannot be asked about, since that
    // division is the one that traps.
    return guarded("I32", cmp_("Eq", ity, A, as_type(wide ? e_i64(-1) : e_i32(-1), ity)),
                   cmp_("Eq", ity, B, as_type(wide ? e_i64(INT64_MIN) : e_i32(INT32_MIN), ity)),
                   and_(cmp_("Ne", ity, A, IZERO),
                        cmp_("Ne", ity, e_binary("Div", ity, RES, A), B)));
#undef A
#undef B
#undef RES
#undef IZERO
  }

  case ND_MEMCPY:
  case ND_MEMSET: {
    // The destination is evaluated once; its address is also the value, which is
    // what C's memcpy and memset return.
    int t = new_temp(PTR_MEM);
    OP(nj_op_localset(out, (uint32_t)t, gen_expr(node->lhs)));
    E *src = gen_expr(node->rhs);
    // A length is a size_t, which is eight bytes wide here, and both engine ops
    // take a 32-bit count.
    E *len = gen_expr(node->cond);
    const char *lty = ir_ty(node->cond->ty);
    if (strcmp(lty, PTR_MEM))
      len = e_convert(lty, PTR_MEM, len);
    if (node->kind == ND_MEMCPY)
      OP(nj_op_memorycopy(out, e_local((uint32_t)t, PTR_MEM), src, len));
    else
      OP(nj_op_memoryfill(out, e_local((uint32_t)t, PTR_MEM), src, len));
    return e_local((uint32_t)t, PTR_MEM);
  }

  // Atomic builtins. The value lives in memory and nowhere else, so each of
  // these is one NJVM-IR atomic op; the result, when there is one, goes to a
  // temp local and is read back from there.
  case ND_ATOMIC_CAS:
  case ND_ATOMIC_CAS_BOOL: {
    int w = atomic_width(node->lhs);
    const char *vty = w == 8 ? "I64" : "I32";
    // The address, the expected value and the replacement are each evaluated
    // once, in that order, before the op.
    E *addr = gen_expr(node->lhs);
    int expected = new_temp(vty);
    OP(nj_op_localset(out, (uint32_t)expected, as_type(gen_expr(node->rhs), vty)));
    E *replacement = as_type(gen_expr(node->cond), vty);
    int old = new_temp(vty);
    OP(nj_op_atomic(out, "Cmpxchg", (uint32_t)w, 0, addr, e_local((uint32_t)expected, vty),
                    replacement, old));
    if (node->kind == ND_ATOMIC_CAS)
      return narrow_atomic_result(as_type(e_local((uint32_t)old, vty), ir_ty(node->ty)),
                                  node->lhs->ty->base);
    // __sync_bool_compare_and_swap: true when the exchange happened, that is,
    // when the value that was there is the one the caller expected.
    int ok = new_temp("I32");
    OP(nj_op_localset(out, (uint32_t)ok,
                      e_binary("Eq", "I32", e_local((uint32_t)old, vty),
                               e_local((uint32_t)expected, vty))));
    return e_local((uint32_t)ok, "I32");
  }

  case ND_ATOMIC_RMW: {
    int w = atomic_width(node->lhs);
    const char *vty = w == 8 ? "I64" : "I32";
    E *addr = gen_expr(node->lhs);
    bool store = !strcmp(node->atomic_op, "Store");
    E *val = store ? e_new(E_CONST_I32, vty) : as_type(gen_expr(node->rhs), vty);
    if (store) {
      // __sync_lock_release(p) stores zero and yields nothing.
      val->val = 0;
      OP(nj_op_atomic(out, "Store", (uint32_t)w, 0, addr, val, NULL, -1));
      return e_new(E_CONST_I32, "I32");
    }
    int old = new_temp(vty);
    OP(nj_op_atomic(out, node->atomic_op, (uint32_t)w, 0, addr, val, NULL, old));
    return narrow_atomic_result(as_type(e_local((uint32_t)old, vty), ir_ty(node->ty)),
                                node->lhs->ty->base);
  }

  case ND_ATOMIC_WAIT: {
    // The width follows the expected value's type: wait32 for a 4-byte one,
    // wait64 for an 8-byte one. A timeout is a nanosecond count and both wasm
    // instructions take an i64.
    E *addr = gen_expr(node->lhs);
    E *expected = gen_expr(node->rhs);
    int w = node->rhs->ty->size;
    const char *vty = w == 8 ? "I64" : "I32";
    expected = as_type(expected, vty);
    E *timeout = as_type(gen_expr(node->cond), "I64");
    int r = new_temp("I32");
    OP(nj_op_atomic(out, "Wait", (uint32_t)w, 0, addr, expected, timeout, r));
    return e_local((uint32_t)r, "I32");
  }

  case ND_ATOMIC_NOTIFY: {
    E *addr = gen_expr(node->lhs);
    E *count = as_type(gen_expr(node->rhs), "I32");
    int r = new_temp("I32");
    OP(nj_op_atomic(out, "Notify", 4, 0, addr, count, NULL, r));
    return e_local((uint32_t)r, "I32");
  }

  case ND_ATOMIC_FENCE:
    OP(nj_op_atomic(out, "Fence", 0, 0, NULL, NULL, NULL, -1));
    return e_new(E_CONST_I32, "I32");

  case ND_MEMORY_SIZE: {
    // memory.size yields an i32 page count; the C type is the eight-byte long.
    int t = new_temp("I32");
    OP(nj_op_localset(out, (uint32_t)t, e_memsize()));
    return e_convert("I32", ir_ty(node->ty), e_local((uint32_t)t, "I32"));
  }

  case ND_MEMORY_GROW: {
    E *delta = as_type(gen_expr(node->lhs), "I32");
    int t = new_temp("I32");
    OP(nj_op_localset(out, (uint32_t)t, e_memgrow(delta)));
    return e_convert("I32", ir_ty(node->ty), e_local((uint32_t)t, "I32"));
  }

  case ND_NOT:
    return e_binary("Eq", "I32", gen_expr(node->lhs), e_i32(0));

  case ND_VAR:
  case ND_MEMBER:
  case ND_DEREF: {
    E *addr = gen_addr(node);
    if (is_addr_value(node->ty))
      return addr; // an array, struct or function is its address
    return gen_load(node, node->ty, addr);
  }

  case ND_ADDR:
    return gen_addr(node->lhs);

  case ND_ASSIGN: {
    if (is_aggregate(node->lhs->ty)) {
      // A struct or union assignment is a byte copy of the object; the value of
      // the expression is the destination, whose address is what an aggregate
      // value is everywhere else in this file.
      E *dst = to_temp(gen_addr(node->lhs));
      E *src = to_temp(gen_expr(node->rhs));
      int size = node->lhs->ty->size;
      OP(nj_op_memorycopy(out, dst, src, e_i32(size)));
      e_free(src);
      return dst;
    }
    // The value of an assignment is a snapshot of what was stored, taken before
    // the store: parse.c's compound-assignment lowering re-reads the lvalue from
    // its own value expression, so returning the tree would read the *new*
    // value again (that is how `++i` once came out as i+2).
    E *addr = to_temp(gen_addr(node->lhs));
    E *val = gen_expr(node->rhs);
    int t = new_temp(val->ty);
    OP(nj_op_localset(out, (uint32_t)t, val));
    e_free(val);
    OP(nj_op_store(out, mem_ty(node->lhs->ty), addr,
                   store_value(e_local((uint32_t)t, local_ty[t]), mem_ty(node->lhs->ty)), 0, 0));
    e_free(addr);
    return e_local((uint32_t)t, local_ty[t]);
  }

  case ND_ADD:
  case ND_SUB:
  case ND_MUL:
  case ND_DIV:
  case ND_MOD:
  case ND_BITAND:
  case ND_BITOR:
  case ND_BITXOR:
  case ND_SHL:
  case ND_SHR:
  case ND_EQ:
  case ND_NE:
  case ND_LT:
  case ND_LE:
    return gen_binary(node);

  case ND_LOGAND:
  case ND_LOGOR:
    return gen_logical(node);

  case ND_COND:
    return gen_cond(node);

  case ND_CAST: {
    // A cast to void throws the value away and keeps the side effect of
    // computing it. Nothing can read the result of such an expression -- C says
    // it has no value -- so a zero stands in for it and the caller drops that.
    if (node->ty->kind == TY_VOID) {
      e_free(gen_expr(node->lhs));
      return e_i32(0);
    }
    if (is_aggregate(node->ty)) {
      // C allows a cast to the same struct or union type and nothing wider, and
      // here the value of an aggregate expression is its address, so the cast
      // is a no-op. Upstream chibicc wraps the right side of every union
      // assignment in one; keeping that shape keeps the AST dump identical to
      // chibicc's, which test/dump-test.sh compares.
      if (node->lhs->ty->kind != node->ty->kind || node->lhs->ty->size != node->ty->size)
        unsupported(node->tok, "a cast between two different struct or union types");
      return gen_expr(node->lhs);
    }
    E *v = gen_expr(node->lhs);
    if (!is_scalar(node->ty))
      unsupported(node->tok, "a cast to a struct or union");
    const char *from = ir_ty(node->lhs->ty);
    const char *to = ir_ty(node->ty);
    if (!strcmp(from, to))
      return v;
    return e_convert(from, to, v);
  }

  case ND_COMMA: {
    E *a = gen_expr(node->lhs);
    OP(nj_op_drop(out, a));
    e_free(a);
    return gen_expr(node->rhs);
  }

  case ND_FUNCALL:
    return gen_call(node);

  case ND_STMT_EXPR: {
    for (Node *n = node->body; n && n->next; n = n->next)
      gen_stmt(n);
    Node *last = node->body;
    while (last && last->next)
      last = last->next;
    if (!last)
      return e_i32(0);
    return gen_expr(last->lhs);
  }

  case ND_LABEL_VAL: {
    E *e = e_new(E_LABELADDR, "I32");
    e->idx = label_id(node->unique_label);
    return e;
  }

  case ND_VLA_PTR:
    unsupported(node->tok, "a VLA");

  default:
    unsupported(node->tok, "this expression");
  }
}

// ───────────────────────── statements ─────────────────────────

static void gen_stmt(Node *node) {
  if (!node)
    return;

  switch (node->kind) {
  case ND_NULL_EXPR:
    return;

  case ND_MEMZERO:
    OP(nj_op_memoryfill(out, frame_addr(node->var->offset), e_i32(0), e_i32(node->var->ty->size)));
    return;

  case ND_EXPR_STMT: {
    E *e = gen_expr(node->lhs);
    OP(nj_op_drop(out, e));
    e_free(e);
    return;
  }

  case ND_BLOCK:
    for (Node *n = node->body; n; n = n->next)
      gen_stmt(n);
    return;

  case ND_RETURN: {
    E *v = node->lhs ? gen_expr(node->lhs) : NULL;
    // The value of an aggregate expression is its address, so returning one
    // is a copy through the hidden buffer pointer — local 0 — and the copy
    // has to happen before the frame goes away.
    bool agg = v && is_aggregate(node->lhs->ty);
    int t = -1;
    if (agg) {
      E *src = to_temp(v); // frees v when it has to make a local
      OP(nj_op_memorycopy(out, e_local(0, PTR_MEM), src, e_i32(node->lhs->ty->size)));
      e_free(src);
    } else if (v) {
      t = new_temp(v->ty);
      OP(nj_op_localset(out, t, v));
      e_free(v);
    }
    if (frame_size)
      OP(nj_op_globalset(out, sp_global,
                         e_binary("Add", "I32", e_local(fp_local, "I32"), e_i32(frame_size))));
    if (agg)
      OP(nj_op_return_void(out));
    else if (t >= 0)
      OP(nj_op_return(out, e_local(t, local_ty[t])));
    else
      OP(nj_op_return_void(out));
    return;
  }

  case ND_IF: {
    uint32_t l_else = fresh_label();
    uint32_t l_end = fresh_label();
    E *c = gen_expr(node->cond);
    OP(nj_op_jmpif(out, e_binary("Eq", "I32", c, e_i32(0)), l_else));
    gen_stmt(node->then);
    OP(nj_op_jmp(out, l_end));
    OP(nj_op_label(out, l_else));
    gen_stmt(node->els);
    OP(nj_op_label(out, l_end));
    return;
  }

  case ND_FOR: {
    // break/continue are already ND_GOTO to this loop's brk/cont labels.
    uint32_t l_begin = fresh_label();
    uint32_t l_cont = label_id(node->cont_label);
    uint32_t l_end = label_id(node->brk_label);
    gen_stmt(node->init);
    OP(nj_op_label(out, l_begin));
    if (node->cond) {
      E *c = gen_expr(node->cond);
      OP(nj_op_jmpif(out, e_binary("Eq", "I32", c, e_i32(0)), l_end));
    }
    gen_stmt(node->then);
    OP(nj_op_label(out, l_cont));
    gen_stmt(node->inc);
    OP(nj_op_jmp(out, l_begin));
    OP(nj_op_label(out, l_end));
    return;
  }

  case ND_DO: {
    uint32_t l_begin = fresh_label();
    uint32_t l_cont = label_id(node->cont_label);
    uint32_t l_end = label_id(node->brk_label);
    OP(nj_op_label(out, l_begin));
    gen_stmt(node->then);
    OP(nj_op_label(out, l_cont));
    {
      E *c = gen_expr(node->cond);
      OP(nj_op_jmpif(out, c, l_begin));
    }
    OP(nj_op_label(out, l_end));
    return;
  }

  case ND_SWITCH: {
    uint32_t l_end = label_id(node->brk_label);
    if (!is_integer(node->cond->ty))
      unsupported(node->tok, "a switch on a non-integer");
    E *sv = to_temp(gen_expr(node->cond));
    int s_local = (int)sv->idx;
    e_free(sv);

    // Each case arm gets a label; dispatch is a comparison chain, so arms stay
    // flat and may fall through into the next one.
    for (Node *n = node->case_next; n; n = n->case_next)
      add_case(n, fresh_label());
    if (node->default_case)
      add_case(node->default_case, fresh_label());

    for (Node *n = node->case_next; n; n = n->case_next) {
      uint32_t id = case_id(n);
      if (n->begin == n->end) {
        OP(nj_op_jmpif(out, e_binary("Eq", "I32", e_local(s_local, "I32"), e_i32((int32_t)n->begin)),
                       id));
      } else {
        E *ge = e_binary("Ge", "I32", e_local(s_local, "I32"), e_i32((int32_t)n->begin));
        E *le = e_binary("Le", "I32", e_local(s_local, "I32"), e_i32((int32_t)n->end));
        OP(nj_op_jmpif(out, e_binary("And", "I32", ge, le), id));
      }
    }
    OP(nj_op_jmp(out, node->default_case ? case_id(node->default_case) : l_end));

    gen_stmt(node->then);
    OP(nj_op_label(out, l_end));
    return;
  }

  case ND_CASE:
    OP(nj_op_label(out, case_id(node)));
    gen_stmt(node->lhs);
    return;

  case ND_GOTO:
      if (!node->unique_label)
      unsupported(node->tok, "a jump to a label the function does not name");
    OP(nj_op_jmp(out, label_id(node->unique_label)));
      return;

  case ND_GOTO_EXPR:
    OP(nj_op_gotoindirect(out, gen_expr(node->lhs)));
    return;

  case ND_LABEL:
    OP(nj_op_label(out, label_id(node->unique_label)));
    gen_stmt(node->lhs);
    return;

  default: {
    // A for-loop's init and inc are bare expressions, not statements, so they
    // reach here. gen_expr rejects the expression kinds it cannot express.
    E *e = gen_expr(node);
    OP(nj_op_drop(out, e));
    e_free(e);
    return;
  }
  }
}

// ───────────────────────── provided symbols ─────────────────────────

// A few names a C source may use are the linker's in a normal toolchain. They
// belong to the module rather than to any translation unit, and their values are
// facts about the layout this compiler chose.
//
//   __heap_base         the first byte of the heap
//   __heap_end          the end of the memory as it starts out, which is where
//                       sbrk continues from
//   __wasm_call_ctors   runs the C++ static constructors; there are none here,
//                       so its body is empty and calling it does nothing
static Obj *heap_base_obj;
static Obj *heap_end_obj;
static long data_end;

static Obj *new_provided_object(const char *name) {
  Obj *o = calloc(1, sizeof(Obj));
  o->name = (char *)name;
  o->is_definition = true;
  o->is_static = true; // not exported: it is not this program's to offer
  o->align = 1;
  o->ty = ty_int; // an address; the size is never used, since it is not laid out
  return o;
}

// The ones that need a function index have to be in the object list, because that
// is what the indices are handed out over and what the bodies are emitted from.
static Obj *new_provided_function(const char *name) {
  Obj *o = new_provided_object(name);
  o->is_function = true;
  o->ty = func_type(ty_void);
  return o;
}

static Obj *add_provided_symbols(Obj *prog) {
  heap_base_obj = new_provided_object("__heap_base");
  heap_end_obj = new_provided_object("__heap_end");
  Obj *ctor = new_provided_function("__wasm_call_ctors");
  ctor->next = heap_base_obj;
  heap_base_obj->next = heap_end_obj;
  heap_end_obj->next = prog;
  return ctor;
}

// ───────────────────────── module ─────────────────────────

// Every object with a static extent gets one address in the data area. The
// thread-local ones come first, so that they form one contiguous block whose base
// and size are known; the rest follow in definition order. One that is never
// initialised writes no data segment: memory starts zeroed, which is what C asks
// for.
static void assign_global_offsets(Obj *prog) {
  for (Obj *var = prog; var; var = var->next) {
    if (var->is_function || !var->is_definition || !var->is_tls)
      continue;
    int align = var->align < 1 ? 1 : var->align;
    if (align > tls_align)
      tls_align = align;
  }

  int top = align_to(GLOBAL_BASE, tls_align);
  tls_base = top;
  for (int pass = 0; pass < 2; pass++) {
    for (Obj *var = prog; var; var = var->next) {
      if (var->is_function || !var->is_definition)
        continue;
      if (var->is_tls != (pass == 0))
        continue;
      int align = var->align < 1 ? 1 : var->align;
      top = align_to(top, align);
      var->offset = top;
      top += var->ty->size;
    }
    if (pass == 0)
      tls_size = top - tls_base;
  }
  data_end = top;

  uint32_t base = align_to(top, 16);
  stack_top = base + STACK_SIZE;
  if (stack_top + MIN_HEAP > MEMORY_END)
    error("the data area leaves no room for a stack of %d bytes and a heap of "
          "%d below the end of memory; raise MEMORY_PAGES",
          STACK_SIZE, MIN_HEAP);
}

// A static initialiser may name an object (int *p = &g;). chibicc records that
// as a relocation with the address left zero; every data address is known by
// now, so the pointer word is written in place. The word is four bytes wide
// because an address is an i32: chibicc's layout reserves eight bytes for a
// pointer and the upper half simply stays zero.
static void resolve_relocations(Obj *prog) {
  for (Obj *var = prog; var; var = var->next) {
    if (var->is_function || !var->is_definition)
      continue;
    for (Relocation *rel = var->rel; rel; rel = rel->next) {
      // parse.c puts a computed-goto label here as its own name, and a label is
      // not an object: the program has no symbol for it.
      Obj *target = find_symbol(*rel->label, var->tu);
      if (!target) {
        note_missing(var->tok, *rel->label);
        continue;
      }
      // A function's value is its index in the function-pointer table, which
      // is assigned from `offset` when the function indices are handed out.
      if (target->is_function)
        indirect_used = true;
      uint32_t word = (uint32_t)(target->offset + rel->addend);
      memcpy(var->init_data + rel->offset, &word, sizeof(word));
    }
  }
}

static void assign_lvar_offsets(Obj *prog) {
  for (Obj *fn = prog; fn; fn = fn->next) {
    if (!fn->is_function || !fn->is_definition)
      continue;
    // `offset` is where the object starts, measured from the frame base, so
    // the object occupies [offset, offset + size) and the frame is large
    // enough by construction.
    int top = 0;
    for (Obj *var = fn->locals; var; var = var->next) {
      int align = var->align < 1 ? 1 : var->align;
      top = align_to(top, align);
      var->offset = top;
      top += var->ty->size;
    }
    fn->stack_size = align_to(top, 16);
  }
}

static void emit_type_array(const char **tys, int n) {
  int a = mp_begin_arr(out);
  for (int i = 0; i < n; i++)
    mp_variant_unit(out, tys[i]);
  mp_end_container(out, a, (size_t)n);
}


static void emit_function(Obj *fn) {
  reset_locals();
  reset_labels();
  body_len = 0;
  frame_size = fn->stack_size;

  // A struct or union return cannot be a wasm value, so the caller passes the
  // address of its buffer as a hidden first parameter, exactly as the C ABI
  // does; param i is then wasm local i + 1.
  bool sret = is_aggregate(fn->ty->return_ty);
  int arg_base = 0;
  if (sret) {
    add_local(PTR_MEM);
    arg_base = 1;
  }
  // chibicc keeps a local for the hidden return-buffer pointer at the head of
  // fn->params when the aggregate is wider than 16 bytes. The parameter itself is
  // wasm parameter 0 (added above), so counting the local again would give the
  // function one more parameter than any caller passes.
  Obj *first_param = fn->params;
  if (sret && fn->ty->return_ty->size > 16 && first_param)
    first_param = first_param->next;
  for (Obj *p = first_param; p; p = p->next)
    add_local(is_aggregate(p->ty) ? PTR_MEM : ir_ty(p->ty));
  // A variadic function's last wasm parameter is the address of the caller's
  // argument buffer; a wasm signature is fixed, so the "..." arguments cannot
  // be parameters of their own.
  int va_param = -1;
  if (takes_varargs(fn->ty))
    va_param = add_local(PTR_MEM);
  nparams = nlocals;
  fp_local = new_temp("I32");

  int f = mp_begin_map(out);
  size_t fields = 0;

  mp_key(out, "body");
  int body = mp_begin_arr(out);

  // Prologue.
  if (frame_size) {
    OP(nj_op_localset(out, (uint32_t)fp_local,
                      e_binary("Sub", "I32", e_global((uint32_t)sp_global, "I32"),
                               e_i32(frame_size))));
    OP(nj_op_globalset(out, (uint32_t)sp_global, e_local((uint32_t)fp_local, "I32")));
  } else {
    OP(nj_op_localset(out, (uint32_t)fp_local, e_global((uint32_t)sp_global, "I32")));
  }
  int i = arg_base;
  for (Obj *p = first_param; p; p = p->next) {
    if (is_aggregate(p->ty)) {
      // The caller passed the address of its own object; C says the callee
      // gets a copy, so the copy happens here. That is also why &param is a
      // per-activation address and why the argument needs no caller-side
      // temporary.
      OP(nj_op_memorycopy(out, frame_addr(p->offset), e_local((uint32_t)i, local_ty[i]),
                          e_i32(p->ty->size)));
    } else {
      OP(nj_op_store(out, mem_ty(p->ty), frame_addr(p->offset),
                     store_value(e_local((uint32_t)i, local_ty[i]), mem_ty(p->ty)), 0, 0));
    }
    i++;
  }
  if (va_param >= 0)
    OP(nj_op_store(out, PTR_MEM, frame_addr(fn->va_buf->offset),
                   e_local((uint32_t)va_param, PTR_MEM), 0, 0));

  // A provided function has no body, only an empty one.
  if (fn->body)
    gen_stmt(fn->body);

  if (fn->ty->return_ty->kind == TY_VOID)
    OP(nj_op_return_void(out));
  else
    OP(nj_op_unreachable(out));

  mp_end_container(out, body, body_len);
  fields++;

  mp_key(out, "results");
  if (fn->ty->return_ty->kind == TY_VOID || sret) {
    // A struct return writes through the hidden parameter and yields nothing.
    int a = mp_begin_arr(out);
    mp_end_container(out, a, 0);
  } else {
    const char *rt = ir_ty(fn->ty->return_ty);
    emit_type_array(&rt, 1);
  }
  fields++;

  mp_key(out, "params");
  emit_type_array(local_ty, nparams);
  fields++;

  mp_key(out, "locals");
  emit_type_array(local_ty + nparams, nlocals - nparams);
  fields++;

  mp_end_container(out, f, fields);
}

void njvm_emit_module(Obj *prog, Mp *m) {
  out = m;
  nsigs = 0;
  indirect_used = false;
  // One object per name, the symbols the linker would provide, then the table
  // that resolves a name to one of them.
  prog = collapse_definitions(prog);
  prog = add_provided_symbols(prog);
  assign_global_offsets(prog);
  heap_base_obj->offset = (int)stack_top;
  heap_end_obj->offset = (int)MEMORY_END;
  build_symbols(prog);
  assign_lvar_offsets(prog);

  // Function indices first: a body may call a function defined later. A static
  // function is still part of the module; it is only left out of the exports.
  //
  // Imported functions take the low indices, because that is the order wasm
  // puts the two groups in: import function k is function index k, and the
  // first function this module defines is index nimports.
  // -1 means "no function index yet". Zero is the default in a calloc'd Obj and
  // is a legitimate index, so it cannot stand for "unassigned" — an alias reads
  // its target's index, and reading it too early would silently name function 0.
  for (Obj *fn = prog; fn; fn = fn->next)
    if (fn->is_function)
      fn->offset = -1;

  nimports = 0;
  uint32_t nfunc = 0;
  for (Obj *fn = prog; fn; fn = fn->next) {
    if (!fn->is_function || !fn->import_module)
      continue;
    if (fn->is_definition)
      error_tok(fn->tok,
                "wnjr-cc: %s carries an import attribute and is defined in this file",
                fn->name);
    if (nimports == import_cap) {
      import_cap = import_cap ? import_cap * 2 : 8;
      imports = realloc(imports, (size_t)import_cap * sizeof *imports);
      if (!imports)
        error("out of memory");
    }
    ImportEnt *ie = &imports[nimports];
    ie->module = fn->import_module;
    ie->field = fn->import_name;
    ie->type_idx = sig_of(fn->tok, fn->ty);
    fn->offset = (int)nimports++;
    nfunc++;
  }
  for (Obj *fn = prog; fn; fn = fn->next) {
    if (!fn->is_function || !fn->is_definition)
      continue;
    fn->offset = (int)nfunc++;
  }

  // An alias is another name for a function of this module: it takes that
  // function's index, and the export table names the same index twice. Resolved
  // by name here, once every definition has its index.
  for (int pass = 0; pass < 8; pass++) {
    bool pending = false;
    for (Obj *fn = prog; fn; fn = fn->next) {
      if (!fn->is_function || !fn->is_alias || fn->offset >= 0)
        continue;
      Obj *target = find_symbol(fn->alias_target, fn->tu);
      if (!target || !target->is_function)
        error_tok(fn->tok, "wnjr-cc: %s is an alias of %s, which is not a function here",
                  fn->name, fn->alias_target);
      if (target->offset < 0) {
        pending = true;
        continue;
      }
      fn->offset = target->offset;
    }
    if (!pending)
      break;
  }

  // After the function indices: a static initialiser may name a function, and
  // the word written for it is that index.
  resolve_relocations(prog);

  int env = mp_begin_map(out);
  mp_key(out, "version");
  mp_uint(out, 1);
  mp_key(out, "module");
  int mod = mp_begin_map(out);
  size_t fields = 0;

  // The imports themselves: the signature index is a wasm type index, which is
  // Module.types' index (the encoder seeds its type section from that list).
  mp_key(out, "imports");
  {
    int a = mp_begin_arr(out);
    for (int i = 0; i < nimports; i++) {
      int im = mp_begin_map(out);
      size_t nif = 0;
      mp_key(out, "module");
      mp_str(out, imports[i].module, strlen(imports[i].module));
      nif++;
      mp_key(out, "name");
      mp_str(out, imports[i].field, strlen(imports[i].field));
      nif++;
      mp_key(out, "kind");
      int kind = mp_variant(out, "Function");
      mp_uint(out, imports[i].type_idx);
      mp_end_container(out, kind, 1);
      nif++;
      mp_end_container(out, im, nif);
    }
    mp_end_container(out, a, (size_t)nimports);
  }
  fields++;

  sp_global = 0;

  mp_key(out, "functions");
  {
    int a = mp_begin_arr(out);
    size_t nf = 0;
    for (Obj *fn = prog; fn; fn = fn->next) {
      if (!fn->is_function || !fn->is_definition)
        continue;
      emit_function(fn);
      nf++;
    }
    mp_end_container(out, a, nf);
  }
  fields++;

  // The types a function pointer can have, collected while the bodies were
  // emitted: Module::types[i] is declared at wasm type index i, which is what
  // CallIndirect's type_idx names.
  mp_key(out, "types");
  {
    int a = mp_begin_arr(out);
    for (int k = 0; k < nsigs; k++) {
      int ft = mp_begin_map(out);
      size_t nft = 0;
      mp_key(out, "params");
      emit_type_array(sigs[k].params, sigs[k].nparams);
      nft++;
      mp_key(out, "results");
      if (sigs[k].result)
        emit_type_array(&sigs[k].result, 1);
      else
        emit_type_array(NULL, 0);
      nft++;
      mp_end_container(out, ft, nft);
    }
    mp_end_container(out, a, (size_t)nsigs);
  }
  fields++;

  // Table 0 is the function-pointer table, and it holds function i at index
  // i, so a C function pointer is just the function's own index.
  mp_key(out, "tables");
  {
    int a = mp_begin_arr(out);
    size_t nt = 0;
    if (indirect_used) {
      int t = mp_begin_map(out);
      mp_key(out, "min");
      mp_uint(out, nfunc);
      mp_key(out, "max");
      mp_nil(out);
      mp_end_container(out, t, 2);
      nt = 1;
    }
    mp_end_container(out, a, nt);
  }
  fields++;

  mp_key(out, "memories");
  {
    int a = mp_begin_arr(out);
    int t = mp_begin_map(out);
    mp_key(out, "min");
    mp_uint(out, MEMORY_PAGES);
    mp_key(out, "max");
    mp_nil(out);
    mp_key(out, "shared");
    mp_bool(out, false);
    mp_end_container(out, t, 3);
    mp_end_container(out, a, 1);
  }
  fields++;

  mp_key(out, "globals");
  {
    int a = mp_begin_arr(out);
    int g = mp_begin_map(out);
    mp_key(out, "ty");
    int gt = mp_begin_map(out);
    mp_key(out, "ty");
    mp_variant_unit(out, "I32");
    mp_key(out, "mutable");
    mp_bool(out, true);
    mp_end_container(out, gt, 2);
    mp_key(out, "init");
    int init = mp_variant(out, "Const");
    int cv = mp_variant(out, "I32");
    mp_int(out, (int)stack_top);
    mp_end_container(out, cv, 1);
    mp_end_container(out, init, 1);
    mp_end_container(out, g, 2);
    mp_end_container(out, a, 1);
  }
  fields++;

  mp_key(out, "tags");
  {
    int a = mp_begin_arr(out);
    mp_end_container(out, a, 0);
  }
  fields++;

  mp_key(out, "exports");
  {
    int a = mp_begin_arr(out);
    size_t ne = 0;
    for (Obj *fn = prog; fn; fn = fn->next) {
      // An alias has an index and a name but no body of its own, and that name
      // is the whole point of it.
      if (!fn->is_function || (!fn->is_definition && !fn->is_alias))
        continue;
      // A static function has no name outside the module, unless an attribute
      // gave it one.
      if (fn->is_static && !fn->export_name)
        continue;
      char *name = fn->export_name ? fn->export_name : fn->name;
      int e = mp_begin_map(out);
      mp_key(out, "name");
      mp_str(out, name, strlen(name));
      mp_key(out, "kind");
      mp_variant_unit(out, "Function");
      mp_key(out, "index");
      mp_uint(out, (uint32_t)fn->offset);
      mp_end_container(out, e, 3);
      ne++;
    }
    mp_end_container(out, a, ne);
  }
  fields++;

  mp_key(out, "start");
  mp_nil(out);
  fields++;

  mp_key(out, "elements");
  {
    int a = mp_begin_arr(out);
    size_t ne = 0;
    if (indirect_used) {
      int e = mp_begin_map(out);
      size_t nf = 0;
      mp_key(out, "mode");
      int mode = mp_variant(out, "Active");
      int act = mp_begin_map(out);
      mp_key(out, "index");
      mp_uint(out, 0);
      mp_key(out, "offset");
      int off = mp_variant(out, "Const");
      int cv = mp_variant(out, "I32");
      mp_int(out, 0);
      mp_end_container(out, cv, 1);
      mp_end_container(out, off, 1);
      mp_end_container(out, act, 2);
      mp_end_container(out, mode, 1);
      nf++;
      mp_key(out, "funcs");
      int fa = mp_begin_arr(out);
      for (uint32_t k = 0; k < nfunc; k++)
        mp_uint(out, k);
      mp_end_container(out, fa, nfunc);
      nf++;
      mp_end_container(out, e, nf);
      ne = 1;
    }
    mp_end_container(out, a, ne);
  }
  fields++;

  mp_key(out, "data");
  {
    int a = mp_begin_arr(out);
    size_t nseg = 0;
    for (Obj *var = prog; var; var = var->next) {
      if (var->is_function || !var->is_definition || !var->init_data)
        continue;
      int seg = mp_begin_map(out);
      mp_key(out, "mode");
      int mode = mp_variant(out, "Active");
      int act = mp_begin_map(out);
      mp_key(out, "index");
      mp_uint(out, 0);
      mp_key(out, "offset");
      int off = mp_variant(out, "Const");
      int cv = mp_variant(out, "I32");
      mp_int(out, var->offset);
      mp_end_container(out, cv, 1);
      mp_end_container(out, off, 1);
      mp_end_container(out, act, 2);
      mp_end_container(out, mode, 1);
      mp_key(out, "bytes");
      int bytes = mp_begin_arr(out);
      for (int i = 0; i < var->ty->size; i++)
        mp_uint(out, (unsigned char)var->init_data[i]);
      mp_end_container(out, bytes, (size_t)var->ty->size);
      mp_end_container(out, seg, 2);
      nseg++;
    }
    mp_end_container(out, a, nseg);
  }
  fields++;

  mp_key(out, "stack_pointer");
  mp_uint(out, (uint32_t)sp_global);
  fields++;

  mp_key(out, "tls_base");
  mp_nil(out);
  fields++;

  mp_end_container(out, mod, fields);
  mp_end_container(out, env, 2);

  // A reference to something nothing defines was given index 0 or address 0
  // above so that the rest of the program could still be checked. Say which, and
  // do not let the module out.
  if (missing.len) {
    for (int i = 0; i < missing.len; i++)
      fprintf(stderr, "wnjr-cc: undefined: %s (used at %s)\n", missing.data[i],
              missing_where.data[i]);
    error("wnjr-cc: %d symbol(s) are used and defined nowhere", missing.len);
  }
}
