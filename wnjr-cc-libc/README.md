# wnjr-cc-libc

wnjr-cc 的 C 标准库，从 [wasi-libc](https://github.com/WebAssembly/wasi-libc) (commit 0d0f8521) 裁剪而来，仅保留 chibicc test 需要的子集，并针对 LP64 指针模型和 wnjr-cc 的 builtin 约定做了适配。

## 来源

- wasi-libc 0d0f8521 (MIT + Apache-2.0 双许可)
- musl libc (MIT)
- dlmalloc (public domain)

## 编译目标

- wasip1-threads（wasm32-wasip1-threads）
- LP64 数据模型（long/pointer 64 位，int 32 位）
- gc 指针标记：高 32 位是 tag，低 32 位是 memory0 地址

## 目录结构

```
wnjr-cc-libc/
├── README.md               # 本文件
├── MANIFEST.md             # 符号清单（符号 → 源文件映射）
├── LICENSE*                # MIT + Apache-2.0
├── include/                # 头文件
│   ├── stdio.h, string.h, stdlib.h, pthread.h, errno.h, ...
│   ├── setjmp.h            # 改写版（builtin 声明）
│   ├── bits/               # alltypes.h, errno.h, pthread.h, ...
│   └── wasi/               # api.h, version.h
└── src/                    # 源文件
    ├── stdio/              # printf.c, vfprintf.c, __stdio_write.c, ...
    ├── string/             # strcmp.c, memcpy.c, ...
    ├── exit/               # exit.c, atexit.c
    ├── thread/             # pthread_create.c, pthread_join.c, ...
    │   ├── wasi-threads/
    │   └── common/
    ├── internal/           # libc.c, floatscan.c, ...
    ├── env/                # __init_tls.c, __stack_chk_fail.c
    ├── bottom-half/        # write.c, writev.c, errno.c, preopens.c, ...
    ├── dlmalloc/           # dlmalloc.c, malloc.c
    └── crt/                # crt1-command.c, crt1-reactor.c
```

## setjmp/longjmp builtin 约定

wnjr-cc 不使用 wasi-libc 原版的 LLVM sjlj 实现（依赖 `-mllvm -wasm-enable-sjlj`），而是映射到 NJVM-IR 的 setjmp/longjmp 操作，再降低到 wasm EH（try_table/throw）。

### 签名

```c
typedef int jmp_buf[1];  // 实际是 i32 标签编号

int  setjmp(jmp_buf env);
void longjmp(jmp_buf env, int val);
```

### 语义

- `setjmp(env)`: 第一次调用返回 0，`longjmp` 返回时返回 `val`（若 `val == 0` 则返回 1）。
- `longjmp(env, val)`: 不返回，跳转到 `setjmp` 点，效果上像 `setjmp` 返回 `val`。

### wnjr-cc 内部处理

1. **setjmp 点**：编译成 `try_table(catch_all $unwind)`，分配一个标签编号（i32）写入 `env`。
2. **longjmp**：编译成 `throw $setjmp_tag, val`，携带返回值 `val`。
3. **捕获**：外层 try_table 捕获后，`br $unwind` 跳到标签，用 `BrTable` 分发多个 setjmp 点。
4. **模块级 tag**：所有 setjmp/longjmp 共用一个 tag（NJVM-IR module.tags）。

### 限制

- `jmp_buf` 只在当前栈帧有效，不可跨函数传递（除非通过指针，且保证目标栈帧仍活跃）。
- 不支持嵌套 longjmp（即从一个 catch handler 里 longjmp 到另一个 setjmp 点）。

## 与 wasi-libc 的差异

1. **数据模型**：wasi-libc 是 ILP32，wnjr-cc-libc 是 LP64。所有指针字段都是 8 字节。
2. **FILE 结构**：musl 的 `FILE` 在 `stdio_impl.h` 里定义，指针字段（`buf`, `wpos`, `wend`, `lock` 等）都改成 8 字节。
3. **pthread 结构**：`pthread_t`, `pthread_attr_t`, `pthread_mutex_t` 等在 `bits/pthread.h` 里定义，指针和 size 字段都改成 8 字节。
4. **__wasi_* 类型**：WASI API 的结构体（`__wasi_iovec_t` 等）在 `wasi/api.h` 里定义，指针��段是 8 字节，但底层 hostcall 时需要截掉高 32 位 tag（按 gc 指针标记约定）。
5. **setjmp.h**：不使用 musl 的 `src/setjmp/wasm32/rt.c`，改用 builtin 声明（见上节）。
6. **assert.h**：不包含 musl 的 `exit/assert.c`，chibicc test 自己实现了 `assert()`。

## L2 后续工作

本 L1 task 只做盘点和搬运，不编译。L2 task 需要：

1. 改写所有 LP64 结构体定义（FILE, pthread_t, __wasi_iovec_t, ...）。
2. 改写 bottom-half 的 hostcall 包装（指针截断：`(uint32_t)(uintptr_t)ptr`）。
3. 用 wnjr-cc 编译所有源文件，修复编译错误（missing headers, incompatible types, ...）。
4. 用 chibicc test 验证（`vendor/chibicc/test/*.c`）。

估计工作量：25~35 小时（见 MANIFEST.md §七）。

## License

MIT + Apache-2.0 双许可，继承自 wasi-libc。见 LICENSE*。
