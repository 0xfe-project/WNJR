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

// Memory layout: everything with a static extent (globals, string literals,
// static locals) lives at [GLOBAL_BASE, global_top); the stack grows down from
// STACK_TOP, which is the end of memory, and must not reach the data area.
#define MEMORY_PAGES 256
#define STACK_TOP 0x1000000
#define GLOBAL_BASE 0x1000
#define MIN_STACK 0x40000

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

// The NJVM-IR type of a C scalar. A pointer is an i32: a wasm32 address.
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
    return ty->size == 8 ? "I64" : "I32";
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
  if (fnty->is_variadic)
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
  if (fnty->is_variadic)
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
      // the value to use. An imported function has an index too.
      if (!node->var->is_definition && !node->var->import_module && !node->var->is_alias)
        unsupported(node->tok, "the address of a function this file does not define");
      indirect_used = true;
      return e_i32(node->var->offset);
    }
    if (!node->var->is_definition)
      unsupported(node->tok, "a reference to a variable this file does not define");
    return e_i32(node->var->offset);
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
static const char *common_ty(E *a, E *b) {
  if (!strcmp(a->ty, "F64") || !strcmp(b->ty, "F64"))
    return "F64";
  if (!strcmp(a->ty, "F32") || !strcmp(b->ty, "F32"))
    return "F32";
  if (!strcmp(a->ty, "I64") || !strcmp(b->ty, "I64"))
    return "I64";
  return "I32";
}

static bool is_comparison(NodeKind kind) {
  return kind == ND_EQ || kind == ND_NE || kind == ND_LT || kind == ND_LE;
}

static E *gen_binary(Node *node) {
  E *a = gen_expr(node->lhs);
  E *b = gen_expr(node->rhs);
  const char *ty = common_ty(a, b);
  a = as_type(a, ty);
  b = as_type(b, ty);
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
  // said where it comes from, or when it is another name for a function here.
  if (!fn->is_definition && !fn->import_module && !fn->is_alias)
    error_tok(fn->tok,
              "wnjr-cc: %s is not defined in this file and carries no import attribute",
              fn->name);
  return (uint32_t)fn->offset;
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
  OP(nj_op_store(out, mem_ty(ty), addr, val, 0, 0));
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
  bool variadic = node->func_ty && node->func_ty->is_variadic;
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
      if (node->ty->size == 8)
        return e_i64(node->val);
      return e_i32((int32_t)node->val);
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
    OP(nj_op_store(out, mem_ty(node->lhs->ty), addr, e_local((uint32_t)t, local_ty[t]), 0, 0));
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

// ───────────────────────── module ─────────────────────────

// Every object with a static extent gets one address in the data area, in
// definition order. One that is never initialised writes no data segment:
// memory starts zeroed, which is what C asks for.
static void assign_global_offsets(Obj *prog) {
  int top = GLOBAL_BASE;
  for (Obj *var = prog; var; var = var->next) {
    if (var->is_function || !var->is_definition)
      continue;
    int align = var->align < 1 ? 1 : var->align;
    top = align_to(top, align);
    var->offset = top;
    top += var->ty->size;
  }
  if (top + MIN_STACK > STACK_TOP)
    error("the data area leaves less than %d bytes of stack below STACK_TOP; "
          "raise MEMORY_PAGES", MIN_STACK);
}

static Obj *find_object(Obj *prog, const char *name) {
  for (Obj *var = prog; var; var = var->next)
    if (var->name && !strcmp(var->name, name))
      return var;
  return NULL;
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
      // parse.c puts a computed-goto label here as its own name.
      Obj *target = find_object(prog, *rel->label);
      if (!target)
        unsupported(var->tok, "a label address in a static initialiser");
      // A function's value is its index in the function-pointer table, which
      // is assigned from `offset` when the function indices are handed out.
      if (target->is_function) {
        if (!target->is_definition && !target->import_module)
          unsupported(var->tok, "a pointer to a function this file does not define");
        indirect_used = true;
        uint32_t word = (uint32_t)(target->offset + rel->addend);
        memcpy(var->init_data + rel->offset, &word, sizeof(word));
        continue;
      }
      if (!target->is_definition)
        unsupported(var->tok, "a pointer to an object this file does not define");
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
  if (fn->ty->is_variadic)
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
                     e_local((uint32_t)i, local_ty[i]), 0, 0));
    }
    i++;
  }
  if (va_param >= 0)
    OP(nj_op_store(out, PTR_MEM, frame_addr(fn->va_buf->offset),
                   e_local((uint32_t)va_param, PTR_MEM), 0, 0));

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
  assign_global_offsets(prog);
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
      Obj *target = NULL;
      for (Obj *t = prog; t; t = t->next)
        if (t->is_function && t->name && !strcmp(t->name, fn->alias_target)) {
          target = t;
          break;
        }
      if (!target)
        error_tok(fn->tok, "wnjr-cc: %s is an alias of %s, which is not a function here",
                  fn->name, fn->alias_target);
      if (!target->is_definition && !target->import_module && !target->is_alias)
        error_tok(fn->tok,
                  "wnjr-cc: %s is an alias of %s, which is not defined in this file",
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
    mp_int(out, STACK_TOP);
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
}
