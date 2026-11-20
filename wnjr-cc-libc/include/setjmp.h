#ifndef _SETJMP_H
#define _SETJMP_H

/* wnjr-cc builtin setjmp/longjmp
 *
 * jmp_buf 是 int (i32)，存储标签编号。
 * setjmp() 第一次返回 0，longjmp 返回时返回 val（若 val==0 则返回 1）。
 * longjmp() 不返回，跳转到 setjmp 点。
 *
 * 实现：wnjr-cc 把 setjmp 映射到 NJVM-IR 的 Op::Setjmp，longjmp 映射到 Op::Longjmp。
 * 底层降低：setjmp → try_table(catch_all $unwind)，longjmp → throw with val。
 * 多个 setjmp 点用 BrTable 分发。
 *
 * 参考 wnjr-cc-libc/README.md 了解详情。
 */

typedef int jmp_buf[1];

int  setjmp(jmp_buf env);
void longjmp(jmp_buf env, int val);

#define setjmp(env)  __builtin_setjmp(env)
#define longjmp(env, val)  __builtin_longjmp(env, val)

#endif
