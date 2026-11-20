// wnjr-cc: minimal type.c subset for tokenize/preprocess
// Based on chibicc by Rui Ueyama (MIT license, see LICENSE)

#include "chibicc.h"

Type *ty_void = &(Type){.kind = TY_VOID, .size = 1, .align = 1};
Type *ty_bool = &(Type){.kind = TY_BOOL, .size = 1, .align = 1};

Type *ty_char = &(Type){.kind = TY_CHAR, .size = 1, .align = 1};
Type *ty_short = &(Type){.kind = TY_SHORT, .size = 2, .align = 2};
Type *ty_int = &(Type){.kind = TY_INT, .size = 4, .align = 4};
Type *ty_long = &(Type){.kind = TY_LONG, .size = 8, .align = 8};

Type *ty_uchar = &(Type){.kind = TY_CHAR, .size = 1, .align = 1, .is_unsigned = true};
Type *ty_ushort = &(Type){.kind = TY_SHORT, .size = 2, .align = 2, .is_unsigned = true};
Type *ty_uint = &(Type){.kind = TY_INT, .size = 4, .align = 4, .is_unsigned = true};
Type *ty_ulong = &(Type){.kind = TY_LONG, .size = 8, .align = 8, .is_unsigned = true};

Type *ty_float = &(Type){.kind = TY_FLOAT, .size = 4, .align = 4};
Type *ty_double = &(Type){.kind = TY_DOUBLE, .size = 8, .align = 8};
Type *ty_ldouble = &(Type){.kind = TY_LDOUBLE, .size = 16, .align = 16};

static Type *new_type(TypeKind kind, int size, int align) {
  Type *ty = calloc(1, sizeof(Type));
  ty->kind = kind;
  ty->size = size;
  ty->align = align;
  return ty;
}

bool is_integer(Type *ty) {
  TypeKind k = ty->kind;
  return k == TY_BOOL || k == TY_CHAR || k == TY_SHORT ||
         k == TY_INT  || k == TY_LONG || k == TY_ENUM;
}

bool is_flonum(Type *ty) {
  return ty->kind == TY_FLOAT || ty->kind == TY_DOUBLE ||
         ty->kind == TY_LDOUBLE;
}

Type *array_of(Type *base, int len) {
  Type *ty = new_type(TY_ARRAY, base->size * len, base->align);
  ty->base = base;
  ty->array_len = len;
  return ty;
}
