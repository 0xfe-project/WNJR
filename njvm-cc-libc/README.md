# njvm-cc-libc

njvm-cc 的 C 库源码, 从 wasi-libc 0d0f85213673682fbfe077c8b9b98a4f549eb1c3 裁出, 只保留 chibicc 测试(`vendor/chibicc/test/*.c` 与 `test/common`)需要的闭包。目标只有 wasm32-wasip1-threads 这一套。

**状态: 只搬运, 没有适配, 没有编译。** 上游文件逐字节原样(`include/setjmp.h` 与 `generated/` 除外)。树里的代码目前不能被 chibicc 编译, 原因和清单见 MANIFEST.md 第 7 节。

- 符号 → 源文件 → 是否搬、没搬的原因、LP64 布局表、L2 工作量估计: 见 MANIFEST.md。
- 不用预编译的 wasi-libc: chibicc 是 LP64(指针 8 字节, gc 指针高 32 位放 tag), wasi-libc 是 ILP32, FILE / iovec / pthread 等布局对不上。
- malloc 保留 dlmalloc。gc_malloc 不在这里。

## 目录

路径保持 wasi-libc 的相对结构, 因为 musl 的 `src/include/*.h` 用 `#include "../../include/x.h"`。

- `libc-top-half/musl/{src,include,arch}`: musl 的源与头。
- `libc-bottom-half/{sources,cloudlibc,crt,headers}`: WASI 主机调用层。
- `dlmalloc/{src,include}`
- `include/setjmp.h`: 改写版, 见下。
- `generated/include/{bits/alltypes.h,wasi/version.h}`: wasi-libc 的 CMake 在构建时生成的两个头, 在 wasm32-wasip1-threads 上生成后原样放入。
- `LICENSE*`(wasi-libc 的 Apache-2.0 WITH LLVM-exception / Apache-2.0 / MIT 三份与总说明)、`libc-top-half/musl/COPYRIGHT`(musl, MIT)、`libc-bottom-half/cloudlibc/LICENSE`(cloudlibc, BSD-2-Clause)。dlmalloc 的 CC0 声明在 `dlmalloc/src/malloc.c` 文件头里。

## setjmp / longjmp

不搬 musl/src/setjmp。musl 的 setjmp.h 在没有 wasm 异常处理时 `#error` 要求 `-mllvm -wasm-enable-sjlj`(LLVM sjlj 依赖), njvm-cc 不走那条路: 引擎已支持 wasm EH, setjmp/longjmp 在 NJVM-IR 里作为 `Op::Setjmp` / `Op::Longjmp`, 由 `wnjr-core/src/njvm/setjmp.rs` 降成 TryTable / Throw。

### builtin 约定

`include/setjmp.h` 声明:

```c
typedef int jmp_buf[1];
int __njvm_setjmp(jmp_buf env);
_Noreturn void __njvm_longjmp(jmp_buf env, int val);
#define setjmp(env)        __njvm_setjmp(env)
#define longjmp(env, val)  __njvm_longjmp(env, val)
```

| builtin | 签名 | njvm-cc 映射到 NJVM-IR |
|---|---|---|
| `__njvm_setjmp` | `int (jmp_buf)` | `Op::Setjmp { buf: <env 地址>, dest: <i32 局部> }` |
| `__njvm_longjmp` | `_Noreturn void (jmp_buf, int)` | `Op::Longjmp { buf: <env 地址>, val: <i32 表达式> }` |

`__wnjr_` 这两个名字是本 task 起的(chibicc 里原来没有同名 builtin, 搜过 vendor/chibicc 和 njvm/)。映射本身由 njvm-cc 实现, 不在本 task 范围内; 在 njvm-cc 实现之前, 这两个函数没有定义, 链接不过。

### 语义(以 njvm/setjmp.rs 的降低为准)

- jmp_buf 只用偏移 0 处的一个 i32: setjmp 点编号, 模块内各点编号互不相同。其余字节不用, 不要往里写别的。
- `setjmp(env)` 第一次返回 0。`longjmp(env, v)` 之后该 setjmp 再次返回 `v`, 若 `v == 0` 返回 1。
- 降低: setjmp/longjmp 共用一个模块级 tag `(label: i32, val: i32)`。longjmp 降成 `throw tag(load.i32 env, v)`; 含 setjmp 的函数被包进 `try_table`, catch 后用 `br_table` 按编号分发到对应 setjmp 点。编号不属于本函数(包括 0, 即未初始化的 jmp_buf)就重新 throw 给调用者, 最终没人接则 trap。
- 局部变量在 longjmp 之后保持最新值(帧还是同一个 wasm 帧), 这是 C 里 "不确定" 的一个合法细化。

### 对 njvm-cc 前端的约束

- `Op::Setjmp` 必须在函数的顶层(平坦 CFG)里, 嵌在结构化 op 里会被 `SetjmpError::NotTopLevel` 拒绝。因此 `if (setjmp(env)) ...` 这类写法要由前端先把 setjmp 提到一个 i32 局部再判断。
- jmp_buf 标识的是 setjmp 点, 不是调用实例。对同一个函数递归, 外层实例的 longjmp 会回到内层实例(njvm/setjmp.rs 注明 "not handled")。
- `__njvm_longjmp` 的 `val` 是 i32。

## LP64 要注意的地方

见 MANIFEST.md 第 6 节。要点: `wasi/wasip1.h` 把 `__wasi_*` 结构体的指针字段写死为 4 字节并带 `_Static_assert`, 那一层要保持 4 字节(调用点按 gc 指针约定取低 32 位地址); musl 与 dlmalloc 的内部结构随 `sizeof(long)`/指针变大; `wasi_thread_start.s` 里有硬编码的 4 字节偏移。
