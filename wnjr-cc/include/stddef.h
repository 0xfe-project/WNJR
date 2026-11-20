// Freestanding compiler header. It answers the __need_* protocol the way the
// compiler-supplied stddef.h of GCC and clang does: a caller that defines
// __need_wint_t and includes this file wants that one name and nothing else.
//
// This matters for wasi-libc: its generated <bits/alltypes.h> asks for wint_t
// that way and then sets __DEFINED_wint_t itself, so a stddef.h that ignored the
// request would leave wint_t undeclared and make every prototype after it in
// <wchar.h> unparseable.
#if defined(__need_wint_t)
typedef unsigned int wint_t;
#undef __need_wint_t
#elif !defined(__STDDEF_H)
#define __STDDEF_H

#define NULL ((void *)0)

typedef unsigned int size_t;
typedef int ssize_t;
typedef long ptrdiff_t;
typedef unsigned int wchar_t;
typedef long max_align_t;

#define offsetof(type, member) ((size_t)&(((type *)0)->member))

#endif
