# wnjr-cc-libc 清单

来源: wasi-libc 0d0f85213673682fbfe077c8b9b98a4f549eb1c3, 目标 wasm32-wasip1-threads。

## 方法

1. 扫描 `vendor/chibicc/test/*.c`(41 个)、`test/common`、`test/test.h`: 去掉注释和字符串后取所有 `ident(`, 减去测试自己定义的函数和宏。剩下的 libc 符号见第 1 节。
2. 用 Homebrew llvm 21 + CMake 把 wasi-libc 在 /tmp 里完整构建一遍(`-DTARGET_TRIPLE=wasm32-wasip1-threads -DSETJMP=OFF -DBUILD_SHARED=OFF`), 对每个 `.obj` 跑 `llvm-nm`, 从第 1 节的符号出发沿 "未定义符号 → 定义它的 .obj" 求闭包(只在 top-half / bottom-half / dlmalloc / bulk-memory / printscan-default 这几个静态目标里找)。得到 **62 个目标文件, 对应 62 个源文件**(第 2 节)。树里共 63 个 .c/.s 文件, 多出的 1 个是被 `dlmalloc.c` 直接 `#include` 的 `dlmalloc/src/malloc.c`。
3. 交叉验证: 写一个用到第 1 节全部符号的 C 程序, 用 `wasm-ld -Map` 真链接 wasi-libc 的 libc.a(`-O0 -fno-builtin`), 取实际被拉进来的 61 个成员。与第 2 步比较, 双向差集为空(第 2 步多出的一个是 crt1-command, 链接时 crt1.o 单独给)。`-O1` 时链接图里少了 memcmp/memcpy/memset/strcmp/strncmp 这 5 个成员(推测是 clang 把调用内联或折叠了, 没有逐个核实), 所以交叉验证改用 `-O0 -fno-builtin`, 这时双向差集为空。
4. 头文件: 对这 62 个源文件逐个用 `clang -M`(原编译参数)取头文件依赖, 再加上测试自己 `#include` 的 stdio/stdlib/pthread/string/errno/stdint/limits。sysroot 里每个头在 wasi-libc 里的出处用逐字节比较确定; 只有 `bits/alltypes.h` 和 `wasi/version.h` 是构建期生成的, 放在 `generated/include/`。
5. 再用 vendor/chibicc 的预处理器检查: 把头文件树复制到 /tmp, 用 chibicc `-M`(chibicc 自己的 include 目录在前)预处理 61 个 .c, 全部通过, 且没有打开 wnjr-cc-libc/ 与 chibicc/include 之外的头。chibicc 自带的 float.h / stdalign.h / stdbool.h / stdnoreturn.h 会遮住 musl 的同名头, 这 4 个加上 `bits/float.h` 一共 5 个文件 chibicc 实际不读, **已删除**。

## 1. 测试实际引用的 libc 符号 → 源文件 → 是否搬

| 符号 | wasi-libc 源文件 | 搬 | 说明 |
|---|---|---|---|
| printf | libc-top-half/musl/src/stdio/printf.c | 是 | 41 个 .c 与 common 都调用(多为 printf("OK\n")) |
| sprintf | libc-top-half/musl/src/stdio/sprintf.c | 是 | function.c, macro.c |
| vsprintf | libc-top-half/musl/src/stdio/vsprintf.c | 是 | function.c, varargs.c |
| (vsnprintf) | libc-top-half/musl/src/stdio/vsnprintf.c | 是 | 间接: sprintf/vsprintf 调用 |
| (vfprintf) | libc-top-half/musl/src/stdio/vfprintf.c | 是 | 间接: printf 系的实现, 浮点格式化在同一文件里 |
| strcmp | libc-top-half/musl/src/string/strcmp.c | 是 | 7 个测试文件 |
| strncmp | libc-top-half/musl/src/string/strncmp.c | 是 | function.c |
| memcmp | libc-top-half/musl/src/string/memcmp.c | 是 | alloca.c, initializer.c, unicode.c |
| strlen | libc-top-half/musl/src/string/strlen.c | 是 | macro.c |
| memcpy | libc-top-half/musl/src/string/memcpy.c | 是 | alloca.c |
| memset | libc-top-half/musl/src/string/memset.c | 是 | 只有 test.h 声明, 41 个 .c 与 common 里没有调用点。仍搬: 它在 test.h 的声明清单里, 体量小 |
| exit | libc-top-half/musl/src/exit/exit.c | 是 | compat.c, common(assert 失败路径) |
| malloc/free/calloc/realloc | dlmalloc/src/dlmalloc.c(它 #include malloc.c) | 是 | 测试不直接调用。任务要求保留 dlmalloc; pthread_create 与 __main_void 也调用 malloc/free/calloc |
| pthread_create | libc-top-half/musl/src/thread/wasi-threads/pthread_create.c | 是 | atomic.c, tls.c |
| pthread_join | libc-top-half/musl/src/thread/wasi-threads/pthread_join.c | 是 | atomic.c, tls.c |
| va_start/va_arg/va_end/va_copy | 无(编译器内置, vendor/chibicc/include/stdarg.h) | 否 | varargs.c, common |
| atomic_compare_exchange_weak, atomic_exchange | 无(vendor/chibicc/include/stdatomic.h) | 否 | atomic.c |
| alloca | 无(chibicc 内置 builtin_alloca) | 否 | alloca.c |
| offsetof | 无(chibicc include/stddef.h 的宏) | 否 | attribute.c, offsetof.c |
| assert | 无(test/common 自己定义) | 否 | test.h 声明 void assert(int,int,char*), 与 <assert.h> 无关 |
| setjmp/longjmp | 无(musl/src/setjmp 不搬) | 否 | 41 个测试和 common 里没有引用。按任务要求改写 setjmp.h, 见 README |

测试 include 的头: stdarg.h stdio.h stddef.h pthread.h stdlib.h stdbool.h stdatomic.h stdalign.h float.h stdnoreturn.h。其中 stdarg/stddef/stdbool/stdatomic/stdalign/float/stdnoreturn 这 7 个由 `vendor/chibicc/include/` 提供, 不从 wasi-libc 搬。

## 2. 搬运的源文件(62 个, 全部来自 nm 闭包, 外加被 include 的 dlmalloc/src/malloc.c)

| 源文件(相对 wasi-libc 根) | 为什么在闭包里 |
|---|---|
| dlmalloc/src/dlmalloc.c | 根: malloc |
| libc-bottom-half/cloudlibc/src/libc/errno/errno.c | errno（被 dlmalloc 引用） |
| libc-bottom-half/cloudlibc/src/libc/sched/sched_yield.c | sched_yield（被 dlmalloc 引用） |
| libc-bottom-half/cloudlibc/src/libc/stdlib/_Exit.c | _Exit（被 exit 引用） |
| libc-bottom-half/cloudlibc/src/libc/sys/uio/writev.c | writev（被 __stdio_write 引用） |
| libc-bottom-half/cloudlibc/src/libc/time/CLOCK_REALTIME.c | _CLOCK_REALTIME（被 pthread_join 引用） |
| libc-bottom-half/cloudlibc/src/libc/time/clock_gettime.c | __clock_gettime（被 futex 引用） |
| libc-bottom-half/cloudlibc/src/libc/unistd/lseek.c | __lseek（被 __stdio_seek 引用） |
| libc-bottom-half/crt/crt1-command.c | 根: _start |
| libc-bottom-half/sources/__errno_location.c | __errno_location（被 sbrk 引用） |
| libc-bottom-half/sources/__main_void.c | __main_void（被 crt1-command 引用） |
| libc-bottom-half/sources/__wasilibc_fd_renumber.c | close（被 __stdio_close 引用） |
| libc-bottom-half/sources/__wasilibc_real.c | __wasi_thread_spawn（被 pthread_create 引用） |
| libc-bottom-half/sources/abort.c | abort（被 vfprintf 引用） |
| libc-bottom-half/sources/isatty.c | __isatty（被 __stdout_write 引用） |
| libc-bottom-half/sources/sbrk.c | sbrk（被 dlmalloc 引用） |
| libc-top-half/musl/src/env/__init_tls.c | __copy_tls（被 pthread_create 引用） |
| libc-top-half/musl/src/errno/strerror.c | strerror（被 vfprintf 引用） |
| libc-top-half/musl/src/exit/exit.c | 根: exit |
| libc-top-half/musl/src/internal/defsysinfo.c | __sysinfo（被 __init_tls 引用） |
| libc-top-half/musl/src/internal/libc.c | __libc（被 pthread_create 引用） |
| libc-top-half/musl/src/locale/__lctrans.c | __lctrans（被 strerror 引用） |
| libc-top-half/musl/src/math/frexp.c | frexp（被 vfprintf 引用） |
| libc-top-half/musl/src/math/scalbn.c | scalbn（被 vfprintf 引用） |
| libc-top-half/musl/src/multibyte/wcrtomb.c | wcrtomb（被 wctomb 引用） |
| libc-top-half/musl/src/multibyte/wctomb.c | wctomb（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/__lockfile.c | __lockfile（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/__stdio_close.c | __stdio_close（被 stdout 引用） |
| libc-top-half/musl/src/stdio/__stdio_exit.c | __stdio_exit_needed（被 __towrite 引用） |
| libc-top-half/musl/src/stdio/__stdio_seek.c | __stdio_seek（被 stdout 引用） |
| libc-top-half/musl/src/stdio/__stdio_write.c | __stdio_write（被 __stdout_write 引用） |
| libc-top-half/musl/src/stdio/__stdout_write.c | __stdout_write（被 stdout 引用） |
| libc-top-half/musl/src/stdio/__towrite.c | __towrite（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/fputs.c | fputs（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/fwrite.c | __fwritex（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/ofl.c | __ofl_lock（被 pthread_create 引用） |
| libc-top-half/musl/src/stdio/printf.c | 根: printf |
| libc-top-half/musl/src/stdio/sprintf.c | 根: sprintf |
| libc-top-half/musl/src/stdio/stderr.c | __stderr_FILE（被 vfprintf 引用） |
| libc-top-half/musl/src/stdio/stdout.c | __stdout_FILE（被 printf 引用） |
| libc-top-half/musl/src/stdio/vfprintf.c | vfprintf（被 printf 引用） |
| libc-top-half/musl/src/stdio/vsnprintf.c | vsnprintf（被 vsprintf 引用） |
| libc-top-half/musl/src/stdio/vsprintf.c | 根: vsprintf |
| libc-top-half/musl/src/string/memchr.c | memchr（被 strnlen 引用） |
| libc-top-half/musl/src/string/memcmp.c | 根: memcmp |
| libc-top-half/musl/src/string/memcpy.c | 根: memcpy |
| libc-top-half/musl/src/string/memset.c | 根: memset |
| libc-top-half/musl/src/string/strcmp.c | 根: strcmp |
| libc-top-half/musl/src/string/strlen.c | 根: strlen |
| libc-top-half/musl/src/string/strncmp.c | 根: strncmp |
| libc-top-half/musl/src/string/strnlen.c | strnlen（被 vfprintf 引用） |
| libc-top-half/musl/src/thread/common/__lock.c | __lock（被 pthread_create 引用） |
| libc-top-half/musl/src/thread/common/__timedwait.c | __timedwait_cp（被 pthread_join 引用） |
| libc-top-half/musl/src/thread/common/__wait.c | __wait（被 pthread_create 引用） |
| libc-top-half/musl/src/thread/common/default_attr.c | __default_guardsize（被 pthread_create 引用） |
| libc-top-half/musl/src/thread/common/pthread_self.c | __wasilibc_pthread_self（被 pthread_create 引用） |
| libc-top-half/musl/src/thread/common/pthread_setcancelstate.c | __pthread_setcancelstate（被 pthread_join 引用） |
| libc-top-half/musl/src/thread/common/pthread_testcancel.c | __pthread_testcancel（被 pthread_join 引用） |
| libc-top-half/musl/src/thread/wasi-threads/futex.c | __wasilibc_futex_wait（被 __wait 引用） |
| libc-top-half/musl/src/thread/wasi-threads/pthread_create.c | 根: pthread_create |
| libc-top-half/musl/src/thread/wasi-threads/pthread_join.c | 根: pthread_join |
| libc-top-half/musl/src/thread/wasi-threads/wasi_thread_start.s | wasi_thread_start（被 pthread_create 引用） |
| dlmalloc/src/malloc.c | 被 dlmalloc.c `#include` |

`vfprintf.c` 按 wasi-libc 默认链接配置(printscan-default: 开浮点, 关 long double)取用, 对应 `-D__wasilibc_printscan_no_long_double`。memcpy.c / memset.c 在 wasi-libc 里带 `-mbulk-memory -DBULK_MEMORY_THRESHOLD=32` 编译, 源码里由 `__wasm_bulk_memory__` 守卫, 不定义它时走普通 C 路径。

## 3. 头文件(98 个)

- `dlmalloc/include`: 1 个
- `generated/include`: 2 个
- `include`: 1 个
- `libc-bottom-half/cloudlibc/src/common`: 3 个
- `libc-bottom-half/cloudlibc/src/include/_`: 1 个
- `libc-bottom-half/crt`: 1 个
- `libc-bottom-half/headers/private/wasi`: 1 个
- `libc-bottom-half/headers/public`: 25 个
- `libc-bottom-half/headers/public/wasi`: 5 个
- `libc-top-half/headers/private`: 1 个
- `libc-top-half/headers/private/wasi`: 3 个
- `libc-top-half/musl/arch/generic/bits`: 1 个
- `libc-top-half/musl/arch/wasm32`: 2 个
- `libc-top-half/musl/arch/wasm32/bits`: 1 个
- `libc-top-half/musl/include`: 23 个
- `libc-top-half/musl/include/sys`: 5 个
- `libc-top-half/musl/src/errno`: 1 个
- `libc-top-half/musl/src/include`: 9 个
- `libc-top-half/musl/src/include/sys`: 1 个
- `libc-top-half/musl/src/internal`: 10 个
- `libc-top-half/musl/src/multibyte`: 1 个

除 `include/setjmp.h`(改写, 第 5 节)与 `generated/include/`(构建期生成)外, 其余每个头都是 clang `-M` 在这 62 个源文件或测试 include 上实际打开的文件, 内容与 wasi-libc 逐字节相同, 路径保持 wasi-libc 的相对结构(musl 的 `src/include/*.h` 用 `#include "../../include/x.h"`)。

编译搜索路径(与 wasi-libc CMake 一致, 把 sysroot 换成 `include` + `generated/include`):
`include`, `libc-bottom-half/headers/public`, `libc-top-half/musl/include`, `libc-top-half/musl/arch/wasm32`, `libc-top-half/musl/arch/generic`, `generated/include`。各编译单元另有 `-I`(`libc-top-half/musl/src/{include,internal}`, `libc-top-half/headers/private`, `libc-bottom-half/cloudlibc/src` 等), 以 wasi-libc 构建出的 compile_commands 为准。

`wasi/api.h` 按 `__wasip1__` 选 `wasip1.h`。它的其它分支 include 的 `wasi/wasip2.h` 和 `wasi/webp2.h` 不在树里, 这些分支在 `__wasip1__` 下不会被预处理。`wasi/wasip3.h` 在树里: `__init_tls.c` 经 `wasi/wasip3_tls.h` 无条件 include 它, 它自己只是个桩, 仅在 `__wasip3__` 下才 include 不在树里的 `__generated_wasip3.h`。

## 4. 没搬的东西

| 项 | 原因 |
|---|---|
| libc-top-half/musl/src/setjmp/*, musl 原版 include/setjmp.h, arch/wasm32/bits/setjmp.h | 任务(3): 依赖 LLVM sjlj。musl 的 setjmp.h 在没有 wasm 异常处理时 `#error` 要求 `-mllvm -wasm-enable-sjlj` |
| musl/src/exit/assert.c 与其它不在闭包里的 musl 源文件 | 不在闭包。测试自己定义 assert |
| musl/src/stdio/vfscanf.c、strtod 系、printscan 的 nofloat/longdouble 变体 | 不在 printf 闭包 |
| crt1-reactor.c | 测试是 command 程序 |
| libc-bottom-half/{clocks,getpid,mman,signal}、fts、emmalloc、dl.c | 不在闭包 |
| wasip2 / webp2 的头, wasip3 的 `__generated_wasip3.h` 及相关源 | 目标只做 wasip1-threads。例外: `wasip3.h`(桩)与 `wasip3_tls.h` 因被 `__init_tls.c` include 而在树里 |
| memmove, strcpy, atexit, fflush, pthread_mutex_*, pthread_attr_*, pthread_detach 等 | 闭包里没有谁引用。上一版提交(8c5b39f)凭猜测列了它们, 这版按 nm 闭包和真实链接核对后删除 |

## 5. setjmp.h

`include/setjmp.h` 是改写版, builtin 约定见 README.md。

## 6. LP64 下布局会变的结构体(给 L2)

方法: 对这 61 个 .c 分别用 clang 的 `-fdump-record-layouts-simple` 在 wasm32 与 wasm64 目标上取布局。wasm64 是 LP64(long 与指针 8 字节), 与 wnjr-cc 的数据模型相同。这是 clang 对 wasm64 的结果, **不是** chibicc 的结果: chibicc 现在解析不了这些源文件(第 7 节), 没法用 chibicc 自己的布局核对。有 22 个编译单元在 wasm32 和 wasm64 两个目标下都没有输出任何布局(原因没有查), 这 22 个里的结构体没有被统计, 所以下表是**下限**。

大小变化(wasm32 → LP64, 字节), 共 24 个:

| 结构 | wasm32 | LP64 | 出现在 |
|---|---|---|---|
| mtx_t | 24 | 40 | pthread_self,errno |
| pthread_attr_t | 36 | 56 | exit,pthread_create |
| pthread_barrier_t | 20 | 32 | exit,pthread_create |
| pthread_mutex_t | 24 | 40 | exit,pthread_create |
| pthread_rwlock_t | 32 | 56 | exit,pthread_create |
| struct _IO_FILE | 128 | 216 | pthread_create,vfprintf |
| struct __libc | 32 | 56 | pthread_create,__lock |
| struct __locale_map | 36 | 48 | strerror,__lctrans |
| struct __locale_struct | 24 | 48 | pthread_create,__lock |
| struct __pthread | 108 | 192 | pthread_join |
| struct __pthread 的匿名子结构 | 12 | 24 | pthread_join |
| struct __wasi_ciovec_t | 8 | 16 | exit,pthread_create |
| struct __wasi_iovec_t | 8 | 16 | exit,pthread_create |
| struct __wasi_prestat_dir_t | 4 | 8 | exit,pthread_create |
| struct __wasi_prestat_t | 8 | 16 | exit,pthread_create |
| struct cookie | 8 | 16 | vsnprintf |
| struct iovec | 8 | 16 | __stdio_write,writev |
| struct malloc_chunk | 16 | 32 | dlmalloc |
| struct malloc_params | 24 | 48 | dlmalloc |
| struct malloc_segment | 16 | 32 | dlmalloc |
| struct malloc_state | 476 | 936 | dlmalloc |
| struct stack_bounds | 8 | 16 | __init_tls |
| struct start_args | 16 | 32 | pthread_create |
| union __wasi_prestat_u_t | 4 | 8 | exit,pthread_create |

大小不变, 共 22 个: pthread_cond_t; struct sched_param 的匿名子结构; struct timespec; union (atomic.h 匿名); union (atomic.h 匿名); struct __wasi_dirent_t; struct __wasi_fdstat_t; struct __wasi_filestat_t; struct __wasi_event_fd_readwrite_t; struct __wasi_event_t; struct __wasi_subscription_clock_t; struct __wasi_subscription_fd_readwrite_t; union __wasi_subscription_u_u_t; struct __wasi_subscription_u_t; struct __wasi_subscription_t; union arg; cnd_t; union (math 匿名, double/u64); union (math 匿名, double/u64); struct errmsgstr_t; struct __mbstate_t; struct timeval。

按层分:

- **WASI 主机 ABI 层(4 字节指针, 必须保持 4 字节)**: `__wasi_iovec_t` `__wasi_ciovec_t` `__wasi_prestat_dir_t` `__wasi_prestat_u_t` `__wasi_prestat_t`(5 个)。`wasi/wasip1.h` 有 `#error` 和 28 条 `_Static_assert`(`_Alignof(void*)==4`, `sizeof(struct __wasi_iovec_t)==8` 等), LP64 下全部失败。这 5 个类型需要用 `uint32_t` 地址字段重写, 调用点(`fd_write` `args_get` `args_sizes_get` 等)要按 gc 指针约定截掉高 32 位 tag。另外 `struct iovec`(musl, LP64 下 16 字节)与 `__wasi_ciovec_t`(8 字节)不再同布局, `writev.c` 里的 `static_assert` 与强转 `(const __wasi_ciovec_t *)iov` 要换成逐项复制。
- **libc 内部结构, 随 sizeof(long)/指针自动变大(musl 与 dlmalloc 多数已按 sizeof 写)**: `_IO_FILE`(128→216) `__pthread`(108→192) `__libc` `__locale_struct` `__locale_map` `cookie`(vsnprintf) `stack_bounds` `malloc_state/chunk/segment/params` 与 pthread_attr/mutex/mtx/cond/rwlock/barrier 的匿名 union。要核对, 不一定改。`__pthread` 里有 "internal (accessed via asm) ABI" 的字段偏移, `pthread_impl.h` 有 `TP_OFFSET`, 需要一起核。
- **硬编码 32 位偏移**: `wasi_thread_start.s` 里 `i32.load 0` / `i32.load 4` 读 `struct start_args` 的 stack 和 tls_base, `start_args` 在 LP64 下 16→32 字节。
- `musl/src/internal/atomic.h` 里 `a_cas_p` 对 `sizeof(void*)==4` 的断言(`a_cas_p_undefined_but_pointer_not_32bit`), wasm64 下 16 个编译单元报错; LP64 下要给 `a_cas_p`/`a_swap_p` 提供 64 位版本。

## 7. chibicc 现在能不能处理这些源文件

没有编译(本 task 不编译)。作为 L3 的输入, 测了预处理和解析:

- 预处理(`chibicc -M`): 61 个 .c 全通过, 前提是把 2 处 `#error ... doesn't ...` 里的单引号换掉(`wasi/wasip1.h:31`, `musl/src/internal/pthread_impl.h:136`)。chibicc 的 tokenizer 对 `#error` 行里的 `'` 报 `unclosed char literal`, 即使该分支没取到(用 5 行小程序复现过)。**这两处在仓库里保持上游原样**; 测试是在 /tmp 的副本上改的。
- 解析(`chibicc -S`): 61 个里 3 个过, 58 个失败。chibicc 遇错即停, 每个文件只有第一个错, 所以下面是下限:
  - `__builtin_va_list`(15 个文件, 经 stdio.h/stdarg.h)
  - `__attribute__((__nothrow__, __leaf__, __nonnull__(...)))`: chibicc 的 `attribute_list` 只认 `packed` 和 `aligned`, 其余 `error: unknown attribute`(10 个文件)
  - `__attribute__((__malloc__, __warn_unused_result__))`(10 个)
  - `__attribute__((__deprecated__(...)))`(15 个)
  - `wasi/wasip1.h` 里的 `_Static_assert(_Alignof(int8_t) == 1, ...)`(7 个文件的第一个错)
  - `__builtin_memcpy`(1 个)
- chibicc 预定义 `__x86_64__` `__linux__` `__ELF__`, 不定义 `__GNUC__`; dlmalloc.c 的 605、2842、2945 行用 `__x86_64__`/`__wasm__` 选分支, L3 编译时要处理。
- 源码用到的 wasm 专用 builtin(L3 要提供): `__builtin_wasm_tls_base/tls_align/tls_size`, `__builtin_wasm_memory_size/grow`, `__builtin_wasm_memory_atomic_wait32/notify`, `__builtin_trap`, `__builtin_unreachable`, `__builtin_ctz/ctzll`, `__builtin_sub_overflow`, `__builtin_memcpy/memset`, `__builtin_assume`, `__sync_val_compare_and_swap`。`__init_tls.c` 里还有 `__asm__(".globaltype __stack_pointer ...")` 读栈指针, `strlen.c`/`memchr.c` 有 wasm SIMD 内联汇编(`__wasm_simd128__` 守卫, 不开 SIMD 就不编)。
- `wasi_thread_start.s` 是 LLVM wasm 汇编语法, chibicc 无法处理, L3 要决定怎么办。

## 8. 给 L2 的工作量估计

范围按任务描述: 改 bottom half 里 LP64 下布局不对的结构体。数目来自第 6 节:

- 必须重写: WASI 主机 ABI 的 5 个类型, `struct iovec` ↔ `__wasi_ciovec_t` 的转换(writev), `start_args` 与 `wasi_thread_start.s` 的硬编码偏移。共 **7 项**, 每项按 1~2 小时(含让 `writev` / `pthread_create` 路径过去), 7~14 小时。
- 核对(可能不改): 第 6 节 "自动变大" 一组约 17 个记录, 逐个比对 chibicc 布局与 clang wasm64 布局并加 `_Static_assert` 钉死: 4~6 小时。这一步需要先有一个能解析这些头的 chibicc。
- `a_cas_p`/`a_swap_p` 64 位版本, 以及 `wasi/wasip1.h` 的 wasm32 断言与 `#error` 的处理: 2~3 小时。

合计约 **13~23 小时**, 置信度低: (a) 没有用 chibicc 实际编过任何一个文件, 第 7 节的解析错误是下限; (b) 布局数字来自 clang wasm64, 有 22 个编译单元没有布局输出(第 6 节), 实际结构体数可能更多; (c) 不含让 chibicc 解析 `__attribute__` / `__builtin_va_list` 的工作, 那属于 wnjr-cc 前端(L3)。上一版写的 "25~35 小时" 是按 "每结构体 1~4 小时" 估的, 没有依据, 作废。

## 9. 已知缺陷

- 上游源文件和头逐字节原样搬入(setjmp.h 与 generated/ 除外), 没有任何 LP64 适配, 树里的代码**还不能**被 chibicc 编译。
- 闭包的前提是 "测试 + 用 wasi-libc 的 printf/malloc/pthread 实现"。若 L3 换别的 stdio, 闭包会小很多。
- `generated/include/bits/alltypes.h` 在 wasm32 目标上生成。其中 pthread 类型已按 `sizeof(long)==8?` 取 64 位分支, 但 `arch/wasm32/bits/alltypes.h.in` 里 `#define _Addr long` 等是为 wasm32 写的, 没有为 LP64 重新生成。
- `MANIFEST.md` 第 6 节的布局表由脚本在构建时统计, 没有随仓库提交脚本; 要复现需按方法 2 构建 wasi-libc 后对每个编译单元加 `-fsyntax-only -Xclang -fdump-record-layouts-simple`。
- 没有跑 `cargo test`: 这次只改了 wnjr-cc-libc/ 下的文件, 没有改 Rust 代码。
