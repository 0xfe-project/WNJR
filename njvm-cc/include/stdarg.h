#ifndef __STDARG_H
#define __STDARG_H

// va_list is a position in the argument list: two counters the register path
// would use, then the two pointers the memory path walks. The type belongs to
// the frontend rather than to this header, because it has to be one type
// everywhere — wasi-libc's <bits/alltypes.h> writes "typedef __builtin_va_list
// va_list", and if the two spellings named different types, a function compiled
// against one could not be called with a va_list built by the other, and the
// complaint would be about struct assignment rather than about anything real.
//
// It is a struct, not a pointer: "va_list ap;" has to be storage for a cursor,
// so that va_copy is a struct copy with a position of its own. musl's vfprintf
// copies its va_list to walk the format twice, and a pointer would leave both
// copies sharing one position.
typedef __builtin_va_list va_list;

// Every argument this frontend passes through "..." is in memory: the caller
// marshals them into a buffer of its own frame and passes the address as one
// more parameter, because a wasm signature is fixed. So a va_list starts with
// both counters spent and only the memory path live.
static __builtin_va_list __va_begin(void *args) {
  __builtin_va_list ap;
  ap.gp_offset = 48;
  ap.fp_offset = 112;
  ap.reg_save_area = 0;
  ap.overflow_arg_area = args;
  return ap;
}

#define va_start(ap, last) ((ap) = __va_begin(__va_buf__))
#define va_end(ap)
#define va_copy(dest, src) ((dest) = (src))

static void *__va_arg_mem(__builtin_va_list *ap, int sz, int align) {
  unsigned long p = (unsigned long)ap->overflow_arg_area;
  if (align > 8)
    p = (p + 15) / 16 * 16;
  ap->overflow_arg_area = (void *)((p + (unsigned long)sz + 7) / 8 * 8);
  return (void *)p;
}

static void *__va_arg_gp(__builtin_va_list *ap, int sz, int align) {
  if (ap->gp_offset >= 48)
    return __va_arg_mem(ap, sz, align);

  void *r = (char *)ap->reg_save_area + ap->gp_offset;
  ap->gp_offset += 8;
  return r;
}

static void *__va_arg_fp(__builtin_va_list *ap, int sz, int align) {
  if (ap->fp_offset >= 112)
    return __va_arg_mem(ap, sz, align);

  void *r = (char *)ap->reg_save_area + ap->fp_offset;
  ap->fp_offset += 8;
  return r;
}

// `ap` is an lvalue of va_list type here, which may itself be a dereference
// (musl writes va_arg(*ap, int)), so the helpers take its address.
#define va_arg(ap, ty)                                                    \
  ({                                                                      \
    int klass = __builtin_reg_class(ty);                                  \
    *(ty *)(klass == 0 ? __va_arg_gp(&(ap), sizeof(ty), _Alignof(ty))      \
            : klass == 1 ? __va_arg_fp(&(ap), sizeof(ty), _Alignof(ty))    \
                         : __va_arg_mem(&(ap), sizeof(ty), _Alignof(ty))); \
  })

// GCC spells the same four operations __builtin_va_something, and musl's printf
// uses those spellings. One implementation, two names.
#define __builtin_va_start(ap, last) va_start(ap, last)
#define __builtin_va_arg(ap, ty) va_arg(ap, ty)
#define __builtin_va_end(ap) va_end(ap)
#define __builtin_va_copy(dest, src) va_copy(dest, src)

#define __GNUC_VA_LIST 1
typedef va_list __gnuc_va_list;

#endif
