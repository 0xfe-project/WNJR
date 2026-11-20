// NJVM-IR emitter: a MessagePack writer, a small expression tree, and the IR
// constructs wnjr-cc needs.
//
// The wire format is the one documented in wnjr-njvm-io/src/lib.rs: every
// struct is a map from field name to value, a fieldless enum variant is its
// name as a string, and any other variant is a one-entry map { "Name": <payload> }.
// Integers use the shortest encoding rmp_serde picks, so a module written here
// is byte-identical to the same module written by the Rust side.
//
// Containers always use the 32-bit header (map32 / array32) so that the entry
// count can be patched after the contents are written; the reader accepts any
// MessagePack width.

#ifndef NJVM_H
#define NJVM_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

//
// MessagePack writer (njvm_mp.c)
//

typedef struct {
  unsigned char *data;
  size_t len;
  size_t cap;
} Mp;

void mp_init(Mp *m);
void mp_free(Mp *m);
void mp_write_file(Mp *m, FILE *out);

// Begin a map/array: writes a 32-bit header with a zero count and returns a
// marker for mp_end_container.
int mp_begin_map(Mp *m);
int mp_begin_arr(Mp *m);
void mp_end_container(Mp *m, int marker, size_t n);

void mp_key(Mp *m, const char *k);
void mp_str(Mp *m, const char *s, size_t len);
void mp_uint(Mp *m, uint64_t v);
void mp_int(Mp *m, int64_t v);
void mp_nil(Mp *m);
void mp_bool(Mp *m, bool v);

// A one-entry map { "name": <payload> }: the payload follows, and the caller
// closes this map with mp_end_container(m, marker, 1).
int mp_variant(Mp *m, const char *name);

// A fieldless enum variant: the bare variant name.
void mp_variant_unit(Mp *m, const char *name);

//
// Expression tree (njvm_ir.c)
//
// Only value-producing nodes live here. Anything with an effect (a call, a
// store) is emitted as an op by codegen and its result read back from a local,
// so an expression can be emitted at the point of use without reordering
// effects.

typedef struct E E;

typedef enum {
  E_LOCAL,      // LocalGet(idx)
  E_GLOBAL,     // GlobalGet(idx)
  E_CONST_I32,  // Const { ty, ConstVal::I32(v) }
  E_CONST_I64,
  E_CONST_F32,
  E_CONST_F64,
  E_BINARY,     // Binary { op, lhs, rhs, ty }
  E_UNARY,      // Unary { op, val, ty }
  E_CONVERT,    // Convert { from, to, val }
  E_LOAD,       // Load { ty, ptr, offset, align }
  E_SELECT,     // Select { cond, then_val, else_val }
  E_LABELADDR,  // LabelAddr(idx): pre-lowering `&&label` marker
} EKind;

struct E {
  EKind kind;
  // IR type name of this expression's value ("I32", "I64", "F32", "F64", "Ptr").
  const char *ty;
  // E_LOCAL, E_GLOBAL
  uint32_t idx;
  // E_CONST_*: the value. F32/F64 hold raw IEEE-754 bits.
  int64_t val;
  // E_BINARY, E_UNARY: operator name ("Add", "Neg", ...).
  const char *op;
  // E_BINARY: the type the operation is performed in. A comparison takes
  // operands of this type and yields I32, so it cannot reuse `ty`.
  const char *op_ty;
  // Operands: E_BINARY uses a and b, E_UNARY/E_CONVERT/E_LOAD use a,
  // E_SELECT uses a (cond), b (then), c (else).
  E *a;
  E *b;
  E *c;
  // E_LOAD
  const char *memty;
  uint32_t offset;
  uint32_t align;
};

E *e_new(EKind kind, const char *ty);
E *e_local(uint32_t idx, const char *ty);
E *e_global(uint32_t idx, const char *ty);
E *e_i32(int32_t v);
E *e_i64(int64_t v);
E *e_f32(uint32_t bits);
E *e_f64(uint64_t bits);
E *e_binary(const char *op, const char *ty, E *a, E *b);
E *e_unary(const char *op, const char *ty, E *a);
E *e_convert(const char *from, const char *to, E *a);
E *e_load(const char *memty, E *ptr, uint32_t offset, uint32_t align);
void e_free(E *e);

void nj_emit_expr(Mp *m, const E *e);

//
// Ops (njvm_ir.c). Expression fields take an E *; the emitter frees nothing.
//

void nj_op_label(Mp *m, uint32_t id);
void nj_op_jmp(Mp *m, uint32_t target);
void nj_op_jmpif(Mp *m, const E *cond, uint32_t target);
void nj_op_brtable(Mp *m, const E *index, const uint32_t *targets, size_t n, uint32_t dflt);
void nj_op_localset(Mp *m, uint32_t idx, const E *val);
void nj_op_globalset(Mp *m, uint32_t idx, const E *val);
void nj_op_store(Mp *m, const char *memty, const E *ptr, const E *val, uint32_t offset, uint32_t align);
void nj_op_memorycopy(Mp *m, const E *dest, const E *src, const E *len);
void nj_op_memoryfill(Mp *m, const E *dest, const E *val, const E *len);
void nj_op_memorygrow(Mp *m, const E *delta);
void nj_op_memorysize(Mp *m);
// Op::Call (result discarded).
void nj_op_call(Mp *m, uint32_t func, E *const *args, size_t nargs);
void nj_op_return(Mp *m, const E *val);
void nj_op_return_void(Mp *m);
void nj_op_drop(Mp *m, const E *val);
// Op::GotoIndirect { target }: the pre-lowering `goto *p` marker.
void nj_op_gotoindirect(Mp *m, const E *target);
// LocalSet { idx, val: Call { func_idx, args } }: a call whose single result
// lands in local `idx`.
void nj_op_localset_call(Mp *m, uint32_t idx, uint32_t func, E *const *args, size_t nargs);
void nj_op_unreachable(Mp *m);
void nj_op_nop(Mp *m);

// Expr::Call as a value (one result).
void nj_expr_call(Mp *m, uint32_t func, E *const *args, size_t nargs);
// Expr::CallIndirect as a value.
void nj_expr_callindirect(Mp *m, uint32_t type_idx, uint32_t table_idx, const E *callee,
                          E *const *args, size_t nargs);

//
// codegen.c
//

typedef struct Obj Obj;

// Translate a parsed translation unit into a complete NJVM-IR module, written
// to `m` as the versioned MessagePack envelope.
void njvm_emit_module(Obj *prog, Mp *m);

#endif
