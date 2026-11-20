# 异常处理（exception handling）

状态：Wasm 3.0 的异常处理在 arm64 上可用——`throw`、`try_table`（`catch` / `catch_all` /
`catch_ref` / `catch_all_ref`）、`throw_ref` 都走真实路径，没有桩。
门禁（本机 arm64 实测）：`cargo test --workspace` **802 passed / 0 failed / 1 ignored**；
spec 全量 **281/281**，其中 `throw.wast`、`try_table.wast`、`throw_ref.wast` 都通过。

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

1. **派发器只挂在 `call_direct` 上，且只有 arm64 后端实现了 `EhHelper`。** x86_64 / riscv64
   上跨调用的 catch 到不了 handler（同函数内 payload 在栈上的静态 catch 不受影响）。
2. **函数内没有真实调用的 `throw_ref`**：tag 静态已知且目标子句只收 exnref（payload 为空）
   时走静态 `Br`；其余情况只能继续向外抛，而函数内没有派发器接它。要让这条路走通，得先有
   「从 exnref 读 payload 字」的运行时 helper——payload 在异常对象里，不在栈上。
3. `catch_ref` 每命中一次登记一个 exnref 句柄，句柄不回收（和 guest 持有其他引用一样）。
   `throw` 本身仍然不分配：异常对象挂在逃逸链上，被 catch 到就丢。
4. 实例不跨线程共享，所以异常也不跨线程传递。
