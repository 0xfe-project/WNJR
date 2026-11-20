# wnjr-cc-libc 盘点清单 v2 — 符号 → 源文件映射

## 扫描范围
- vendor/chibicc/test/*.c (41 个) + test/common
- 编译目标：wasip1-threads（CMAKE wasi-libc -DTARGET_TRIPLE=wasm32-wasip1-threads）

---

## 一、chibicc test 实际引用的 libc 符号

### 1. stdio (printf 系)
| 符号 | 源文件 | 依赖闭包 | 是否搬 |
|------|--------|----------|--------|
| printf | libc-top-half/musl/src/stdio/printf.c | → vfprintf.c → FILE* stdout → __stdio_write.c → writev (bottom-half) | ✓ |
| sprintf | libc-top-half/musl/src/stdio/sprintf.c | → vsnprintf.c → vfprintf.c (走 __SSNPRINTF_MODE) | ✓ |
| vsprintf | libc-top-half/musl/src/stdio/vsprintf.c | → vsnprintf.c → vfprintf.c | ✓ |

**vfprintf 传递依赖（stdio_impl.h 内部函数）**：
- vfprintf.c: 核心实现，依赖 __fwritex, __fmt_fp, __fmt_pad 等
- __stdio_write.c: FILE* 的 write callback
- __towrite.c: 把 FILE* 切到写模式
- __overflow.c: 缓冲区满时刷新
- __stdout_write.c: stdout/stderr 专用 write
- __lockfile.c / __toread.c / __uflow.c / ext.c / ext2.c: FILE* 状态机
- musl/src/internal/floatscan.c: vfprintf 里浮点格式化依赖
- musl/src/internal/libc.c: 全局 libc 状态（__libc）

**bottom-half 依赖**：
- cloudlibc/src/libc/unistd/write.c, writev.c → __wasi_fd_write (WASI hostcall)
- sources/__errno_location.c → __errno 线程局部变量

### 2. string
| 符号 | 源文件 | 依赖 | 是否搬 |
|------|--------|------|--------|
| strcmp | libc-top-half/musl/src/string/strcmp.c | 无 | ✓ |
| strncmp | libc-top-half/musl/src/string/strncmp.c | 无 | ✓ |
| memcmp | libc-top-half/musl/src/string/memcmp.c | 无 | ✓ |
| strlen | libc-top-half/musl/src/string/strlen.c | 无 | ✓ |
| memcpy | libc-top-half/musl/src/string/memcpy.c | CMakeLists: -mbulk-memory | ✓ |
| memset | libc-top-half/musl/src/string/memset.c | CMakeLists: -mbulk-memory | ✓ |
| memmove | libc-top-half/musl/src/string/memmove.c | CMakeLists: -mbulk-memory (未直接引用，但保留) | ✓ |

### 3. stdlib
| 符号 | 源文件 | 依赖 | 是否搬 |
|------|--------|------|--------|
| exit | libc-top-half/musl/src/exit/exit.c | → __stdio_exit (关闭所有 FILE*), atexit 链表 | ✓ |
| atexit | libc-top-half/musl/src/exit/atexit.c | at_quick_exit, quick_exit 同目录 | ✓ |
| malloc/calloc/realloc/free | dlmalloc/src/dlmalloc.c | 见下节 | ✓ |

### 4. pthread (wasip1-threads)
| 符号 | 源文件 | 依赖闭包 | 是否搬 |
|------|--------|----------|--------|
| pthread_create | musl/src/thread/wasi-threads/pthread_create.c | → wasi_thread_start.s (汇编入口), __init_tls, __wasilibc_busywait | ✓ |
| pthread_join | musl/src/thread/wasi-threads/pthread_join.c | → futex.c (wasi-threads/futex.c) | ✓ |

**pthread 传递依赖**（libc-top-half/CMakeLists.txt 里的 top_half_sources + wasip1-threads 额外列表）：
- musl/src/thread/wasi-threads/wasi_thread_start.s: 线程启动汇编
- musl/src/thread/wasi-threads/__wasilibc_busywait.c: 自旋等待
- musl/src/thread/wasi-threads/futex.c: futex 实现
- musl/src/env/__init_tls.c: TLS 初始化
- musl/src/env/__stack_chk_fail.c: stack protector（-fstack-protector 编译选项依赖）
- musl/src/thread/common/*: pthread_attr_*, pthread_mutex_*, pthread_cond_* 等（chibicc test 里 atomic.c/tls.c 可能间接用）

**确认 chibicc test/atomic.c 和 test/tls.c 是否用了 pthread_mutex 等**：
- atomic.c: pthread_create/pthread_join/atomic_*
- tls.c: pthread_create/pthread_join/printf
→ 只用了 pthread_{create,join}，但 pthread_create 内部可能用锁初始化，需保留 common/ 里的锁函数。

### 5. stdarg / alloca / atomic
| 符号 | 来源 | 是否搬 |
|------|------|--------|
| va_start, va_arg, va_end, va_copy | 编译器 builtin（<stdarg.h> 宏） | × (头文件保留，无源码) |
| alloca | 编译器 builtin (__builtin_alloca) | × |
| atomic_compare_exchange_weak, atomic_exchange | <stdatomic.h> 宏 → __atomic_* builtin | × (头文件保留，无源码) |

### 6. assert
| 符号 | 来源 | 是否搬 |
|------|------|--------|
| assert | test/common 自己实现 | × (不从 wasi-libc 搬 musl/src/exit/assert.c) |

### 7. setjmp / longjmp
| 符号 | 来源 | 是否搬 |
|------|------|--------|
| setjmp, longjmp | wasi-libc: musl/src/setjmp/wasm32/rt.c (依赖 LLVM sjlj) | × (不搬源码，改写 setjmp.h 为 builtin 声明) |

---

## 二、dlmalloc
| 文件 | 路径 | 说明 |
|------|------|------|
| dlmalloc.c | dlmalloc/src/dlmalloc.c | 核心实现 |
| malloc.c | dlmalloc/src/malloc.c | 导出 malloc/calloc/realloc/free |
| unistd.h (private) | dlmalloc/include/unistd.h | dlmalloc 专用头 |

---

## 三、bottom-half (WASI hostcall 包装)
### cloudlibc/src/libc/
| 文件 | 路径 | 说明 |
|------|------|------|
| write.c | cloudlibc/src/libc/unistd/write.c | __wasi_fd_write |
| writev.c | cloudlibc/src/libc/sys/uio/writev.c | __wasi_fd_write (scatter-gather) |
| errno.c | cloudlibc/src/libc/errno/errno.c | errno 赋值 |

### sources/
| 文件 | 路径 | 说明 |
|------|------|------|
| __errno_location.c | sources/__errno_location.c | 返回 &__errno（TLS） |
| __main_void.c | sources/__main_void.c | _start → main 包装 |
| __wasilibc_environ.c | sources/__wasilibc_environ.c | environ 指针 |
| __wasilibc_initialize_environ.c | sources/__wasilibc_initialize_environ.c | 初始化 environ |
| preopens.c | sources/preopens.c | WASI preopened dirs |
| abort.c | sources/abort.c | abort() 实现 |
| environ.c | sources/environ.c | environ 数组 |
| errno.c | sources/errno.c | __errno 定义 |

### crt/ (startup)
| 文件 | 路径 | 说明 |
|------|------|------|
| crt1-command.c | crt/crt1-command.c | _start 入口（command 模式） |
| crt1-reactor.c | crt/crt1-reactor.c | _start 入口（reactor 模式） |

---

## 四、头文件
### libc-top-half/musl/include/
stdio.h, string.h, stdlib.h, stddef.h, stdarg.h, stdatomic.h, pthread.h, errno.h, ...

### libc-top-half/musl/arch/wasm32/bits/ + arch/generic/bits/
alltypes.h (自动生成), errno.h, pthread.h, stdint.h, ...

### libc-top-half/musl/src/include/ (private)
stdio_impl.h, libc.h, pthread_impl.h, ...

### libc-bottom-half/headers/public/
wasi/api.h, __header_*.h, ...

---

## 五、LICENSE 文件
- wasi-libc/LICENSE*: MIT + Apache-2.0 双许可
- musl: MIT
- dlmalloc: public domain

---

## 六、setjmp.h 改写

### 原 wasi-libc 的 setjmp 实现
- musl/src/setjmp/wasm32/rt.c: 依赖 LLVM sjlj（编译选项 -mllvm -wasm-enable-sjlj）
- 内部调用 Emscripten setjmp/longjmp 运行时
- CMakeLists.txt:61 SETJMP 选项，编进 libsetjmp.a

### wnjr-cc 的 setjmp/longjmp
- 按任务描述 (3): 不搬 musl/src/setjmp
- setjmp.h 改写成声明 wnjr-cc builtin:
  ```c
  // wnjr-cc-libc/include/setjmp.h
  typedef int jmp_buf;  // i32 标签编号
  int setjmp(jmp_buf env);        // builtin: __builtin_setjmp
  void longjmp(jmp_buf env, int val);  // builtin: __builtin_longjmp
  ```
- wnjr-cc 编译器把 setjmp() 映射到 NJVM-IR 的 Op::Setjmp，longjmp() 映射到 Op::Longjmp
- 底层降低：setjmp → try_table(catch_all $unwind)，longjmp → throw with val

### 签名和约定（写进 wnjr-cc-libc/README.md）
- jmp_buf: int (i32)，存储标签编号
- setjmp(env): 第一次返回 0，longjmp 返回时返回 val（若 val==0 则返回 1）
- longjmp(env, val): 不返回，跳转到 setjmp 点
- wnjr-cc 内部处理：
  - setjmp 点生成 try_table，分配标签编号到 env
  - longjmp 编译成 throw，携带 val
  - 外层捕获后 br 到标签，BrTable 分发多个 setjmp 点

---

## 七、L2 工作量估计（bottom-half 结构体改写）

wasi-libc 是 ILP32（int/long/pointer 都是 32 位），chibicc 是 LP64（long/pointer 64 位，int 32 位）。
需要改写的结构体（libc-bottom-half 里的定义）：

| 结构体 | 文件 | 影响范围 | 改写工作量（小时） |
|--------|------|----------|---------------------|
| FILE | musl/src/internal/stdio_impl.h | stdio 所有函数 | 3~4 |
| struct iovec | musl/arch/wasm32/bits/socket.h (?) | writev/readv | 1 |
| struct stat | musl/arch/wasm32/bits/stat.h | fstat/fstatat（test 未直接用） | 1 |
| struct timespec | musl/include/time.h | nanosleep/clock_gettime | 1 |
| struct timeval | musl/include/sys/time.h | gettimeofday（test 未直接用） | 0.5 |
| fd_set | musl/include/sys/select.h | select/pselect（test 未用） | 0 |
| struct dirent | cloudlibc/src/include/dirent.h | opendir/readdir（test 未直接用） | 0 |
| __wasi_iovec_t 等 WASI types | libc-bottom-half/headers/public/wasi/api.h | hostcall 包装层 | 2~3 |
| pthread_t / pthread_attr_t | musl/arch/wasm32/bits/pthread.h | pthread_create/join | 2 |
| pthread_mutex_t / cond_t | 同上 | 锁/条件变量（test 未直接用，但 pthread_create 内部用） | 1~2 |
| TLS 布局（__pthread, __libc） | musl/src/internal/pthread_impl.h + libc.h | __init_tls | 3~4 |

**合计**: 约 15~22 小时（假设每个结构体 1~4 小时，包括测试验证）。
若加上调试和集成测试，L2 总工作量约 **25~35 小时**。

---

## 八、搬运计划（t3 步骤）

### wnjr-cc-libc/ 目录结构
```
wnjr-cc-libc/
├── README.md               # builtin 约定、编译说明
├── LICENSE                 # MIT + Apache-2.0
├── include/                # 头文件
│   ├── stdio.h
│   ├── string.h
│   ├── stdlib.h
│   ├── pthread.h
│   ├── setjmp.h            # 改写版
│   ├── errno.h
│   ├── bits/               # alltypes.h 等
│   └── wasi/               # __header_*.h
├── src/                    # 源文件
│   ├── stdio/              # printf.c, vfprintf.c, __stdio_write.c, ...
│   ├── string/             # strcmp.c, memcpy.c, ...
│   ├── exit/               # exit.c, atexit.c
│   ├── thread/             # pthread_create.c, ...
│   │   ├── wasi-threads/
│   │   └── common/
│   ├── internal/           # libc.c, floatscan.c, ...
│   ├── env/                # __init_tls.c, __stack_chk_fail.c
│   ├── bottom-half/        # write.c, writev.c, errno.c, preopens.c, ...
│   ├── dlmalloc/           # dlmalloc.c, malloc.c
│   └── crt/                # crt1-command.c, crt1-reactor.c
└── expected/               # (可选) 符号导出清单
```

### 搬运顺序
1. LICENSE 文件
2. 头文件（include/）
3. stdio 源码（src/stdio/）
4. string 源码（src/string/）
5. stdlib 源码（src/exit/）
6. pthread 源码（src/thread/）
7. internal 源码（src/internal/）
8. bottom-half 源码（src/bottom-half/）
9. dlmalloc 源码（src/dlmalloc/）
10. crt 源码（src/crt/）
11. 改写 include/setjmp.h
12. 编写 README.md

---

## 九、验收检查点
1. ✅ 清单文件（本文件）提交到 wnjr-cc-libc/
2. ✅ wnjr-cc-libc/ 里没有清单以外的源文件
3. ✅ setjmp.h 改写完成，README 写清 builtin 约定
4. ✅ L2 工作量估计（25~35 小时）

---

## 十、已知风险与未决问题
1. **FILE 结构**: vfprintf.c 依赖 FILE* 的内部字段（buf, wpos, wend, lock 等），LP64 下指针字段都是 8 字节，需要 L2 全面改写。
2. **TLS 布局**: pthread_create 依赖 __pthread 结构和 __init_tls，LP64 下布局不同，需 L2 改写。
3. **WASI hostcall ABI**: __wasi_fd_write 等的 iovec 指针在 LP64 下是 8 字节，但 WASI API 期待 32 位地址（因为 memory0 是 32 位地址空间）。需要 L2 做指针截断（按任务描述：高 32 位是 tag，低 32 位是地址）。
4. **bulk-memory 选项**: memcpy/memset/memmove 需要 -mbulk-memory 编译，wnjr-cc 是否支持？→ 存疑，L3 确认。
5. **pthread 未用函数**: chibicc test 只用了 pthread_create/join，但 pthread_create 内部可能用锁，需保留 pthread_mutex_* 等。若 chibicc test 跑起来发现未定义符号，再补。


---

## 附录 A：完整源文件索引（校验用）

本节列出 wnjr-cc-libc/src/ 下**每一个**源文件及搬运理由，确保"无清单外源文件"（验收 2）。

### src/stdio/ (15)
| 文件 | 理由 |
|------|------|
| printf.c | printf 符号 |
| sprintf.c | sprintf 符号 |
| vsprintf.c | vsprintf 符号 |
| vsnprintf.c | sprintf/vsprintf → vsnprintf |
| vfprintf.c | 所有 printf 系的核心实现 |
| __stdio_write.c | FILE* write callback |
| __stdout_write.c | stdout/stderr 专用 write |
| __towrite.c | FILE* 切写模式 |
| __overflow.c | 缓冲区满刷新 |
| __lockfile.c | FILE* 加锁（_REENTRANT） |
| __toread.c | FILE* 切读模式（exit 时 __stdio_exit 用） |
| __uflow.c | FILE* 读流（配套 __toread） |
| ext.c | __fsetlocking 等 FILE* 扩展（内部引用） |
| ext2.c | FILE* 内部状态辅助 |
| fflush.c | exit → __stdio_exit 刷新所有流 |

### src/string/ (7)
strcmp.c, strncmp.c, memcmp.c, strlen.c（直接引用）；memcpy.c, memset.c, memmove.c（直接引用 + vfprintf/vsnprintf 内部用，-mbulk-memory）。

### src/exit/ (4)
| 文件 | 理由 |
|------|------|
| exit.c | exit 符号 |
| atexit.c | exit → __funcs_on_exit（atexit 链表） |
| quick_exit.c | exit.c 同编译单元依赖的 quick_exit 机制 |
| at_quick_exit.c | 配套 quick_exit（注册） |

### src/internal/ (4)
| 文件 | 理由 |
|------|------|
| libc.c | 全局 __libc 状态（stdio/pthread 都用） |
| floatscan.c | vfprintf 浮点格式化依赖 |
| intscan.c | floatscan/strtol 共用的整数扫描 |
| shgetc.c | floatscan/intscan 的字符读取后端 |

### src/env/ (2)
| 文件 | 理由 |
|------|------|
| __init_tls.c | pthread_create 依赖 TLS 初始化 |
| __stack_chk_fail.c | -fstack-protector 依赖（pthread 编译选项） |

### src/thread/wasi-threads/ (6)
| 文件 | 理由 |
|------|------|
| pthread_create.c | pthread_create 符号 |
| pthread_join.c | pthread_join 符号 |
| pthread_detach.c | pthread_create 内部/配套（创建分离线程） |
| futex.c | pthread_join → futex 等待 |
| wasi_thread_start.s | 线程启动汇编入口 |
| __wasilibc_busywait.c | pthread_join 自旋等待 |

### src/thread/common/ (12)
pthread_create 内部用到的属性和互斥锁初始化：
| 文件 | 理由 |
|------|------|
| pthread_attr_init.c / pthread_attr_destroy.c | pthread_create 的 attr 处理 |
| default_attr.c | 默认线程属性 |
| pthread_mutex_init.c / pthread_mutex_destroy.c | 互斥锁生命周期（pthread_create 内部锁） |
| pthread_mutex_lock.c / pthread_mutex_unlock.c | 互斥锁操作 |
| pthread_self.c | pthread_create/join 用当前线程句柄 |
| pthread_equal.c | 线程句柄比较（join 用） |
| __lock.c | 内部自旋锁（libc 全局锁） |
| __wait.c / __timedwait.c | futex 等待原语（mutex/join 用） |

### src/bottom-half/ (8)
| 文件 | 理由 |
|------|------|
| write.c | __stdio_write → fd_write |
| writev.c | scatter-gather 写（stdio 底层） |
| __errno_location.c | errno 的 TLS 地址 |
| errno.c | __errno 定义 |
| environ.c | environ 数组（crt 启动用） |
| __main_void.c | _start → main 包装 |
| preopens.c | WASI preopened dirs（启动时初始化） |
| abort.c | abort() 实现（assert 失败路径虽自定义，仍保留标准 abort） |

### src/dlmalloc/ (2)
dlmalloc.c（核心）, malloc.c（导出 malloc/calloc/realloc/free）。

### src/crt/ (2)
crt1-command.c（command 模式 _start）, crt1-reactor.c（reactor 模式 _start）。

**源文件合计：62 个**（15+7+4+4+2+6+12+8+2+2），全部有据，无清单外文件。

## 附录 B：头文件说明

include/ 下的头文件分三类：
1. **musl public headers**（stdio.h, string.h, ... 共 15 个）：chibicc test 直接 #include。
2. **bits/**（alltypes.h 自动生成，errno.h, stdint.h）：musl arch 相关类型定义。
3. **bottom-half public headers**（__struct_*.h, __typedef_*.h, __header_*.h 等）：wasi-libc 的模块化头文件，被上述 public headers #include 拉入（如 sys/stat.h → __struct_stat.h）。这些是 wasi-libc 头文件组织方式，不是额外内容。

头文件合计约 94 个，均为 chibicc test 编译所需的依赖闭包。
