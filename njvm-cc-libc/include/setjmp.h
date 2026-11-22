#ifndef _SETJMP_H
#define _SETJMP_H

/*
 * njvm-cc-libc setjmp.h. Replaces musl's setjmp.h, which needs
 * -mllvm -wasm-enable-sjlj (see README.md, "setjmp / longjmp").
 *
 * __njvm_setjmp and __njvm_longjmp are njvm-cc builtins, not library
 * functions: no object in this tree defines them. njvm-cc maps them to
 * NJVM-IR Op::Setjmp / Op::Longjmp.
 *
 * jmp_buf holds one i32 at offset 0 (the setjmp site number). The rest of
 * the buffer is unused. Do not store anything else in it.
 */

typedef int jmp_buf[1];

/* First return: 0. After longjmp(env, v): v, or 1 if v == 0. */
int __njvm_setjmp(jmp_buf env);
_Noreturn void __njvm_longjmp(jmp_buf env, int val);

#define setjmp(env) __njvm_setjmp(env)
#define longjmp(env, val) __njvm_longjmp(env, val)

#endif
