// NJVM-IR constructs as MessagePack: the expression tree and the ops.
// See njvm.h and the wire format documented in wnjr-njvm-io/src/lib.rs.

#include "chibicc.h"
#include "njvm.h"

#include <string.h>

// ───────────────────────── expression tree ─────────────────────────

static E *e_alloc(EKind kind, const char *ty) {
  E *e = calloc(1, sizeof(E));
  if (!e)
    error("out of memory");
  e->kind = kind;
  e->ty = ty;
  return e;
}

E *e_new(EKind kind, const char *ty) {
  return e_alloc(kind, ty);
}

E *e_local(uint32_t idx, const char *ty) {
  E *e = e_alloc(E_LOCAL, ty);
  e->idx = idx;
  return e;
}

E *e_global(uint32_t idx, const char *ty) {
  E *e = e_alloc(E_GLOBAL, ty);
  e->idx = idx;
  return e;
}

E *e_i32(int32_t v) {
  E *e = e_alloc(E_CONST_I32, "I32");
  e->val = v;
  return e;
}

E *e_i64(int64_t v) {
  E *e = e_alloc(E_CONST_I64, "I64");
  e->val = v;
  return e;
}

E *e_f32(uint32_t bits) {
  E *e = e_alloc(E_CONST_F32, "F32");
  e->val = bits;
  return e;
}

E *e_f64(uint64_t bits) {
  E *e = e_alloc(E_CONST_F64, "F64");
  e->val = (int64_t)bits;
  return e;
}

// The comparisons of the IR (BinaryOp in wnjr-core/src/njvm/verify.rs). wasm has
// no separate unsigned comparison instruction: these take both operands in the
// named type, which is what the signedness of that type selects, and answer an
// i32 whatever it is.
static int is_cmp_op(const char *op) {
  return !strcmp(op, "Eq") || !strcmp(op, "Ne") || !strcmp(op, "Lt") ||
         !strcmp(op, "Le") || !strcmp(op, "Gt") || !strcmp(op, "Ge");
}

E *e_binary(const char *op, const char *ty, E *a, E *b) {
  // The third argument reaches the wire as the operand type, op_ty. The
  // expression's own type is what the instruction leaves on the stack, so a
  // comparison is an i32 and everything else computes in the named type. Without
  // this a comparison of two unsigned words claimed to be a U32, and every
  // consumer that trusted that type planted a value of the wrong one.
  E *e = e_alloc(E_BINARY, is_cmp_op(op) ? "I32" : ty);
  e->op = op;
  e->op_ty = ty;
  e->a = a;
  e->b = b;
  return e;
}

E *e_unary(const char *op, const char *ty, E *a) {
  E *e = e_alloc(E_UNARY, ty);
  e->op = op;
  e->a = a;
  return e;
}

E *e_convert(const char *from, const char *to, E *a) {
  E *e = e_alloc(E_CONVERT, to);
  e->op = from;
  e->a = a;
  return e;
}

E *e_load(const char *memty, E *ptr, uint32_t offset, uint32_t align) {
  // A Load produces exactly the type its MemType names (verify.rs: load_type),
  // so a narrow load must be converted before use as an int.
  E *e = e_alloc(E_LOAD, memty);
  e->memty = memty;
  e->a = ptr;
  e->offset = offset;
  e->align = align;
  return e;
}

E *e_memsize(void) {
  return e_alloc(E_MEMSIZE, "I32");
}

E *e_memgrow(E *delta) {
  // memory.grow yields the previous page count, or -1.
  E *e = e_alloc(E_MEMGROW, "I32");
  e->a = delta;
  return e;
}

void e_free(E *e) {
  if (!e)
    return;
  e_free(e->a);
  e_free(e->b);
  e_free(e->c);
  free(e);
}

// ───────────────────────── expression emission ─────────────────────────

static void const_val(Mp *m, const char *kind, int64_t v, bool raw_bits) {
  int mk = mp_variant(m, kind);
  if (raw_bits)
    mp_uint(m, (uint64_t)v);
  else
    mp_int(m, v);
  mp_end_container(m, mk, 1);
}

void nj_emit_expr(Mp *m, const E *e) {
  switch (e->kind) {
  case E_LOCAL: {
    int mk = mp_variant(m, "LocalGet");
    mp_uint(m, e->idx);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_GLOBAL: {
    int mk = mp_variant(m, "GlobalGet");
    mp_uint(m, e->idx);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_CONST_I32:
  case E_CONST_I64:
  case E_CONST_F32:
  case E_CONST_F64: {
    int mk = mp_variant(m, "Const");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "ty");
    mp_variant_unit(m, e->ty);
    n++;
    mp_key(m, "val");
    switch (e->kind) {
    case E_CONST_I32:
      const_val(m, "I32", e->val, false);
      break;
    case E_CONST_I64:
      const_val(m, "I64", e->val, false);
      break;
    case E_CONST_F32:
      const_val(m, "F32", e->val, true);
      break;
    default:
      const_val(m, "F64", e->val, true);
      break;
    }
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_BINARY: {
    // Both operands are read in the type the operation names (op_ty), so that is
    // the type each of them has to have.
    if (strcmp(e->op_ty, e->a->ty) || strcmp(e->op_ty, e->b->ty))
      error("internal error: %s in %s with operands of %s and %s", e->op, e->op_ty, e->a->ty,
            e->b->ty);
    int mk = mp_variant(m, "Binary");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "op");
    mp_variant_unit(m, e->op);
    n++;
    mp_key(m, "lhs");
    nj_emit_expr(m, e->a);
    n++;
    mp_key(m, "rhs");
    nj_emit_expr(m, e->b);
    n++;
    mp_key(m, "ty");
    mp_variant_unit(m, e->op_ty ? e->op_ty : e->ty);
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_UNARY: {
    int mk = mp_variant(m, "Unary");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "op");
    mp_variant_unit(m, e->op);
    n++;
    mp_key(m, "val");
    nj_emit_expr(m, e->a);
    n++;
    mp_key(m, "ty");
    mp_variant_unit(m, e->ty);
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_CONVERT: {
    // A conversion names the type it starts from, and that has to be the child's
    // type: this is the check that would have caught a value of one type being
    // converted from another, which the verifier only sees after the fact and
    // without a source location.
    if (strcmp(e->op, e->a->ty))
      error("internal error: convert from %s of a %s expression", e->op, e->a->ty);
    int mk = mp_variant(m, "Convert");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "from");
    mp_variant_unit(m, e->op);
    n++;
    mp_key(m, "to");
    mp_variant_unit(m, e->ty);
    n++;
    mp_key(m, "val");
    nj_emit_expr(m, e->a);
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_LOAD: {
    int mk = mp_variant(m, "Load");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "ty");
    mp_variant_unit(m, e->memty);
    n++;
    mp_key(m, "ptr");
    nj_emit_expr(m, e->a);
    n++;
    mp_key(m, "offset");
    mp_uint(m, e->offset);
    n++;
    mp_key(m, "align");
    mp_uint(m, e->align);
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_MEMSIZE: {
    // MemorySize is a fieldless variant.
    mp_variant_unit(m, "MemorySize");
    return;
  }
  case E_MEMGROW: {
    int mk = mp_variant(m, "MemoryGrow");
    int f = mp_begin_map(m);
    mp_key(m, "delta");
    nj_emit_expr(m, e->a);
    mp_end_container(m, f, 1);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_LABELADDR: {
    int mk = mp_variant(m, "LabelAddr");
    mp_uint(m, e->idx);
    mp_end_container(m, mk, 1);
    return;
  }
  case E_SELECT: {
    int mk = mp_variant(m, "Select");
    int f = mp_begin_map(m);
    size_t n = 0;
    mp_key(m, "cond");
    nj_emit_expr(m, e->a);
    n++;
    mp_key(m, "then_val");
    nj_emit_expr(m, e->b);
    n++;
    mp_key(m, "else_val");
    nj_emit_expr(m, e->c);
    n++;
    mp_end_container(m, f, n);
    mp_end_container(m, mk, 1);
    return;
  }
  }
  error("internal error at %s:%d", __FILE__, __LINE__);
}

// ───────────────────────── ops ─────────────────────────

// A struct-like op variant: { name: { <fields> } }.
static int op_variant(Mp *m, const char *name, int *fields) {
  int mk = mp_variant(m, name);
  *fields = mp_begin_map(m);
  return mk;
}

static void op_end(Mp *m, int mk, int fields, size_t n) {
  mp_end_container(m, fields, n);
  mp_end_container(m, mk, 1);
}

void nj_op_label(Mp *m, uint32_t id) {
  int fields;
  int mk = op_variant(m, "Label", &fields);
  size_t n = 0;
  mp_key(m, "id");
  mp_uint(m, id);
  n++;
  mp_key(m, "params");
  int a = mp_begin_arr(m);
  mp_end_container(m, a, 0);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_jmp(Mp *m, uint32_t target) {
  int fields;
  int mk = op_variant(m, "Jmp", &fields);
  mp_key(m, "target");
  mp_uint(m, target);
  op_end(m, mk, fields, 1);
}

void nj_op_jmpif(Mp *m, const E *cond, uint32_t target) {
  int fields;
  int mk = op_variant(m, "JmpIf", &fields);
  size_t n = 0;
  mp_key(m, "cond");
  nj_emit_expr(m, cond);
  n++;
  mp_key(m, "target");
  mp_uint(m, target);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_brtable(Mp *m, const E *index, const uint32_t *targets, size_t n, uint32_t dflt) {
  int fields;
  int mk = op_variant(m, "BrTable", &fields);
  size_t f = 0;
  mp_key(m, "index");
  nj_emit_expr(m, index);
  f++;
  mp_key(m, "targets");
  int a = mp_begin_arr(m);
  for (size_t i = 0; i < n; i++)
    mp_uint(m, targets[i]);
  mp_end_container(m, a, n);
  f++;
  mp_key(m, "default");
  mp_uint(m, dflt);
  f++;
  op_end(m, mk, fields, f);
}

void nj_op_localset(Mp *m, uint32_t idx, const E *val) {
  int fields;
  int mk = op_variant(m, "LocalSet", &fields);
  size_t n = 0;
  mp_key(m, "idx");
  mp_uint(m, idx);
  n++;
  mp_key(m, "val");
  nj_emit_expr(m, val);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_globalset(Mp *m, uint32_t idx, const E *val) {
  int fields;
  int mk = op_variant(m, "GlobalSet", &fields);
  size_t n = 0;
  mp_key(m, "idx");
  mp_uint(m, idx);
  n++;
  mp_key(m, "val");
  nj_emit_expr(m, val);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_store(Mp *m, const char *memty, const E *ptr, const E *val, uint32_t offset, uint32_t align) {
  int fields;
  int mk = op_variant(m, "Store", &fields);
  size_t n = 0;
  mp_key(m, "ty");
  mp_variant_unit(m, memty);
  n++;
  mp_key(m, "ptr");
  nj_emit_expr(m, ptr);
  n++;
  mp_key(m, "val");
  nj_emit_expr(m, val);
  n++;
  mp_key(m, "offset");
  mp_uint(m, offset);
  n++;
  mp_key(m, "align");
  mp_uint(m, align);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_memorycopy(Mp *m, const E *dest, const E *src, const E *len) {
  int fields;
  int mk = op_variant(m, "MemoryCopy", &fields);
  size_t n = 0;
  mp_key(m, "dest");
  nj_emit_expr(m, dest);
  n++;
  mp_key(m, "src");
  nj_emit_expr(m, src);
  n++;
  mp_key(m, "len");
  nj_emit_expr(m, len);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_memoryfill(Mp *m, const E *dest, const E *val, const E *len) {
  int fields;
  int mk = op_variant(m, "MemoryFill", &fields);
  size_t n = 0;
  mp_key(m, "dest");
  nj_emit_expr(m, dest);
  n++;
  mp_key(m, "val");
  nj_emit_expr(m, val);
  n++;
  mp_key(m, "len");
  nj_emit_expr(m, len);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_atomic(Mp *m, const char *op, uint32_t width, uint32_t offset, const E *ptr,
                  const E *val, const E *replacement, int32_t dest) {
  int fields;
  int mk = op_variant(m, "Atomic", &fields);
  size_t n = 0;
  mp_key(m, "op");
  mp_variant_unit(m, op);
  n++;
  mp_key(m, "width");
  mp_uint(m, width);
  n++;
  mp_key(m, "offset");
  mp_uint(m, offset);
  n++;
  // An absent operand is an explicit nil, which is the None of the IR's
  // Option fields; so is an absent destination.
  mp_key(m, "ptr");
  if (ptr)
    nj_emit_expr(m, ptr);
  else
    mp_nil(m);
  n++;
  mp_key(m, "val");
  if (val)
    nj_emit_expr(m, val);
  else
    mp_nil(m);
  n++;
  mp_key(m, "replacement");
  if (replacement)
    nj_emit_expr(m, replacement);
  else
    mp_nil(m);
  n++;
  mp_key(m, "dest");
  if (dest < 0)
    mp_nil(m);
  else
    mp_uint(m, (uint64_t)dest);
  n++;
  op_end(m, mk, fields, n);
}

void nj_op_memorygrow(Mp *m, const E *delta) {
  int fields;
  int mk = op_variant(m, "MemoryGrow", &fields);
  mp_key(m, "delta");
  nj_emit_expr(m, delta);
  op_end(m, mk, fields, 1);
}

void nj_op_memorysize(Mp *m) {
  mp_variant_unit(m, "MemorySize");
}

static void call_fields(Mp *m, uint32_t func, E *const *args, size_t nargs, size_t *pn) {
  size_t n = 0;
  mp_key(m, "func_idx");
  mp_uint(m, func);
  n++;
  mp_key(m, "args");
  int a = mp_begin_arr(m);
  for (size_t i = 0; i < nargs; i++)
    nj_emit_expr(m, args[i]);
  mp_end_container(m, a, nargs);
  n++;
  *pn = n;
}

void nj_op_call(Mp *m, uint32_t func, E *const *args, size_t nargs) {
  int fields;
  int mk = op_variant(m, "Call", &fields);
  size_t n;
  call_fields(m, func, args, nargs, &n);
  op_end(m, mk, fields, n);
}

void nj_expr_call(Mp *m, uint32_t func, E *const *args, size_t nargs) {
  int mk = mp_variant(m, "Call");
  int fields = mp_begin_map(m);
  size_t n;
  call_fields(m, func, args, nargs, &n);
  op_end(m, mk, fields, n);
}

static void indirect_fields(Mp *m, uint32_t type_idx, uint32_t table_idx, const E *callee,
                            E *const *args, size_t nargs, size_t *pn) {
  size_t n = 0;
  mp_key(m, "type_idx");
  mp_uint(m, type_idx);
  n++;
  mp_key(m, "table_idx");
  mp_uint(m, table_idx);
  n++;
  mp_key(m, "callee");
  nj_emit_expr(m, callee);
  n++;
  mp_key(m, "args");
  int a = mp_begin_arr(m);
  for (size_t i = 0; i < nargs; i++)
    nj_emit_expr(m, args[i]);
  mp_end_container(m, a, nargs);
  n++;
  *pn = n;
}

void nj_expr_callindirect(Mp *m, uint32_t type_idx, uint32_t table_idx, const E *callee,
                          E *const *args, size_t nargs) {
  int mk = mp_variant(m, "CallIndirect");
  int fields = mp_begin_map(m);
  size_t n;
  indirect_fields(m, type_idx, table_idx, callee, args, nargs, &n);
  op_end(m, mk, fields, n);
}

void nj_op_return(Mp *m, const E *val) {
  int fields;
  int mk = op_variant(m, "Return", &fields);
  mp_key(m, "vals");
  int a = mp_begin_arr(m);
  if (val)
    nj_emit_expr(m, val);
  mp_end_container(m, a, val ? 1 : 0);
  op_end(m, mk, fields, 1);
}

void nj_op_return_void(Mp *m) {
  nj_op_return(m, NULL);
}

void nj_op_drop(Mp *m, const E *val) {
  int fields;
  int mk = op_variant(m, "Drop", &fields);
  mp_key(m, "val");
  nj_emit_expr(m, val);
  op_end(m, mk, fields, 1);
}

void nj_op_gotoindirect(Mp *m, const E *target) {
  int mk = mp_variant(m, "GotoIndirect");
  int fields = mp_begin_map(m);
  mp_key(m, "target");
  nj_emit_expr(m, target);
  mp_end_container(m, fields, 1);
  mp_end_container(m, mk, 1);
}

void nj_op_localset_call(Mp *m, uint32_t idx, uint32_t func, E *const *args, size_t nargs) {
  int mk = mp_variant(m, "LocalSet");
  int fields = mp_begin_map(m);
  size_t n = 0;
  mp_key(m, "idx");
  mp_uint(m, idx);
  n++;
  mp_key(m, "val");
  nj_expr_call(m, func, args, nargs);
  n++;
  mp_end_container(m, fields, n);
  mp_end_container(m, mk, 1);
}

void nj_op_callindirect(Mp *m, uint32_t type_idx, uint32_t table_idx, const E *callee,
                         E *const *args, size_t nargs) {
  int fields;
  int mk = op_variant(m, "CallIndirect", &fields);
  size_t n;
  indirect_fields(m, type_idx, table_idx, callee, args, nargs, &n);
  op_end(m, mk, fields, n);
}

void nj_op_localset_callindirect(Mp *m, uint32_t idx, uint32_t type_idx, uint32_t table_idx,
                                 const E *callee, E *const *args, size_t nargs) {
  int mk = mp_variant(m, "LocalSet");
  int fields = mp_begin_map(m);
  size_t n = 0;
  mp_key(m, "idx");
  mp_uint(m, idx);
  n++;
  mp_key(m, "val");
  nj_expr_callindirect(m, type_idx, table_idx, callee, args, nargs);
  n++;
  mp_end_container(m, fields, n);
  mp_end_container(m, mk, 1);
}

void nj_op_unreachable(Mp *m) {
  mp_variant_unit(m, "Unreachable");
}

void nj_op_nop(Mp *m) {
  mp_variant_unit(m, "Nop");
}
