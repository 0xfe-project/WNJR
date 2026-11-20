#ifndef _SETJMP_H
#define _SETJMP_H

/*
 * wnjr-cc-libc setjmp.h. Replaces musl's setjmp.h, which needs
 * -mllvm -wasm-enable-sjlj (see README.md, "setjmp / longjmp").
 *
 * __wnjr_setjmp and __wnjr_longjmp are wnjr-cc builtins, not library
 * functions: no object in this tree defines them. wnjr-cc maps them to
 * NJVM-IR Op::Setjmp / Op::Longjmp.
 *
 * jmp_buf holds one i32 at offset 0 (the setjmp site number). The rest of
 * the buffer is unused. Do not store anything else in it.
 */

typedef int jmp_buf[1];

/* First return: 0. After longjmp(env, v): v, or 1 if v == 0. */
int __wnjr_setjmp(jmp_buf env);
_Noreturn void __wnjr_longjmp(jmp_buf env, int val);

#define setjmp(env) __wnjr_setjmp(env)
#define longjmp(env, val) __wnjr_longjmp(env, val)

#endif
