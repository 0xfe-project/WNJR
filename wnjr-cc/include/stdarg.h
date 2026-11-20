#ifndef __STDARG_H
#define __STDARG_H

// va_list points into the save area a variadic call leaves behind: two counters
// into that area, then the two pointers the va_arg helpers walk.
//
// The type belongs to the frontend rather than to this header, because it has to
// be one type everywhere. wasi-libc's <bits/alltypes.h> writes
// "typedef __builtin_va_list va_list" and this header writes the same thing; if
// the two differed, a function compiled against one spelling could not be called
// with a va_list produced by the other, and the complaint would be about struct
// assignment rather than about anything real. It is a pointer, not GCC's array of
// one, because what va_list denotes here is a position in the area: va_start
// points it at the area, va_copy copies the pointer.
typedef __builtin_va_list va_list;

#define va_start(ap, last) ((ap) = (__builtin_va_list)__va_area__)
#define va_end(ap)
#define va_copy(dest, src) ((dest) = (src))

static void *__va_arg_mem(__builtin_va_list ap, int sz, int align) {
  void *p = ap->overflow_arg_area;
  if (align > 8)
    p = (p + 15) / 16 * 16;
  ap->overflow_arg_area = ((unsigned long)p + sz + 7) / 8 * 8;
  return p;
}

static void *__va_arg_gp(__builtin_va_list ap, int sz, int align) {
  if (ap->gp_offset >= 48)
    return __va_arg_mem(ap, sz, align);

  void *r = ap->reg_save_area + ap->gp_offset;
  ap->gp_offset += 8;
  return r;
}

static void *__va_arg_fp(__builtin_va_list ap, int sz, int align) {
  if (ap->fp_offset >= 112)
    return __va_arg_mem(ap, sz, align);

  void *r = ap->reg_save_area + ap->fp_offset;
  ap->fp_offset += 8;
  return r;
}

#define va_arg(ap, ty)                                                  \
  ({                                                                    \
    int klass = __builtin_reg_class(ty);                                \
    *(ty *)(klass == 0 ? __va_arg_gp(ap, sizeof(ty), _Alignof(ty)) :    \
            klass == 1 ? __va_arg_fp(ap, sizeof(ty), _Alignof(ty)) :    \
            __va_arg_mem(ap, sizeof(ty), _Alignof(ty)));                \
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
