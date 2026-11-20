# 异常处理（exception handling）

状态：Wasm 3.0 的异常处理在 arm64、x86_64、riscv64 三个后端上都有实现：`throw`、`try_table`
（`catch` / `catch_all` / `catch_ref` / `catch_all_ref`）、`throw_ref` 走真实路径，没有桩。
三个后端的验证程度不同，按架构分开写：

| 架构 | 怎么跑的 | 结果 |
|---|---|---|
| arm64 | 本机原生，`cargo test --workspace` | **803 passed / 0 failed / 1 ignored**；spec 全量 **281/281**（`throw.wast`、`try_table.wast`、`throw_ref.wast` 通过） |
| x86_64 | musl 静态交叉编译，在 amd64 容器里经 qemu-x86_64 跑，不是原生硬件 | 全 workspace **764 passed / 0 failed**（加入递归 setjmp 测试前实测）；加入后重跑 `wnjr-core` lib **612 passed / 0 failed**，`njvm::setjmp` 20 条全过。spectest 没跑 |
| riscv64 | musl 静态交叉编译，经 qemu-riscv64 跑，只跑 `wnjr-core` lib | **552 passed / 1 failed**，`njvm::setjmp` 20 条全过。失败的是 `njvm::erase::lower_tests::trivial_memory_grow_size_nop_drop_fence`，原因是 riscv64 原子指令未实现，与异常处理无关。spectest 没跑；`wnjr-wasi` / `wnjr-cli` 在 riscv64 上编不过（见 THREADS.md） |

riscv64 的 lib 测试是在源码副本上编的：副本去掉了 `wnjr-core` 对 `wnjr-wasi` 的 dev-dependency，
把两处 `expect_err` 改成 `.err().expect()`（`Module` 没有 `Debug`，riscv64 的 cfg 分支编不过），
并去掉 `-C panic=abort`（测试不支持 abort）。仓库本身没有为此改动。

## 两条路径

1. **静态**：`throw` 的 catch 子句和它在同一个函数里，中间没有真实调用，decode 直接生成
   `Br` 到 handler 标签（`decode.rs` `handle_throw` → `match_throw_to_catch`）。
2. **动态**：调用点落在某个 `try_table` 作用域里时，`attach_eh_dispatch`
   (`jit/machine/lower_module.rs`) 在调用点之后合成一个派发器：逐个 tag 比较，命中就把
   payload 交给 handler，都不命中就 `rethrow` 交给外层。payload 上限 `EH_MAX_PAYLOAD = 4` 个字。

## payload 怎么到 handler 手上

handler 块有两个入口通道，`finish_eh_dispatch` 两个都认：

- **块参数寄存器**：handler 直接收值（`LinearValue` 参数）。异常的 payload 从栈底算起，
  `catch_ref` 的 exnref 在最上面，所以它是最后一个参数。
- **handler 自己开头的 load**：payload 绑进普通 local 时，handler 的第一条指令就是
  `load <slot>` 再把它写进 cell。那串 load 的地址就是落点，值是倒序写进去的（wasm 栈先弹栈顶）。

一个被 catch 的块必须保留自己的入口：`cleanup` 既不合并也不消除 handler 块
（`is_eh_handler`）。合并以后 handler 的入口退到前驱开头——对 fallthrough 形 handler 就是
抛出那段代码——于是「抛出 → 派发 → 跳回开头 → 再抛出」，死循环。

## catch_ref

- **静态**：`AllocExnRef { tag_idx, arity }`，payload 从栈上对应的 frame slot 读，
  走 `EH_ALLOC_EXN_REF` 的 `start_slot` / `slot_count`。arity 为 0 和不为 0 是同一条路，
  不再出现 `ref.null` 顶替 exnref。
- **动态**：派发器为 handler 的最后一个值物化 exnref（`EhHelperOp::ExnRef` →
  `EH_PENDING_EXN_REF`）。它把待处理异常登记进 `SharedRefRegistry` 一次，guest 拿到的就是
  普通 exnref 句柄；handler 把它绑进 local 或直接 `throw_ref` 都成立。
- 一个 `_ref` 子句不再让整个 `try_table` 失去派发器（以前 `attach_eh_dispatch` 见到
  `forwards_exn` 就整体放弃，连同一张表里的普通 `catch` 一起废掉）。

## 已知限制

1. **派发器只挂在 `call_direct` 上。** `call_indirect` 路径上的 throw 不经过派发器。三个后端
   的 `lower_call` 都按三态处理返回码（0 成功 / 1 trap / 2 `NativeCallStatus::Thrown` 走 thrown 边），
   `EhHelper` 都已实现。x86_64 和 riscv64 的实现只经 qemu 验证，没有在原生硬件上跑过。
2. **函数内没有真实调用的 `throw_ref`**：tag 静态已知且目标子句只收 exnref（payload 为空）
   时走静态 `Br`；其余情况只能继续向外抛，而函数内没有派发器接它。要让这条路走通，得先有
   「从 exnref 读 payload 字」的运行时 helper——payload 在异常对象里，不在栈上。
3. `catch_ref` 每命中一次登记一个 exnref 句柄，句柄不回收（和 guest 持有其他引用一样）。
   `throw` 本身仍然不分配：异常对象挂在逃逸链上，被 catch 到就丢。
4. 实例不跨线程共享，所以异常也不跨线程传递。
5. **setjmp 经递归调用时 longjmp 会落在最内层活动记录。** jmp_buf 只记录 site 号，不记录
   activation。`f(1)` 调 `f(0)`，`f(0)` 向 `f(1)` 的 buffer longjmp 时，C 语义是回到 `f(1)`，实际回到
   `f(0)`。`njvm::setjmp::tests::recursive_longjmp_resumes_the_innermost_activation` 把现状（返回
   9007，C 语义应为 107）钉住；以后返回 107 说明限制已消除，要同时改这条、`setjmp.rs` 模块文档和
   BLUE 文档。
6. `raise_exception` 构造的 `ErrorRepr::Exception` 里 `exn` 是 `RefValue::null()`，只用于让
   `finish_preserved_result` 映射成 `Thrown` 状态，不得作为公开返回值交给嵌入者；给宿主的 exnref 由
   `PendingEscape::into_error` 生成。
