# Threads 设计

状态：th1–th10 已实现。`cargo test --workspace`：arm64 **802 passed / 0 failed / 1 ignored**（本机实测）；
x86_64 本轮未复跑，只保证 `cargo check --offline --target x86_64-unknown-linux-gnu` 通过。
spec 全量 281 个 wast 文件在 arm64 上 281 通过（含 `proposals/threads/atomic.wast`）；
官方 wasi-threads testsuite 14 个用例全过。对应实现步骤 th1–th10。

## 结论

实例不跨线程共享。每线程一个 Instance，线程之间只共享两样东西：编译产物（Module）和 shared memory。热路径上没有任何锁。

不用 `Arc<Mutex<Instance>>`。代价不是 mutex 本身，是 cache line bouncing：每个线程读写同一把锁的 cache line 和 `RefCell` borrow flag 的 cache line 会来回传，热路径上的普通访存也会碰到。

## 语义依据

- threads 提案里只有 memory 可以是 shared 的。table、global、GC 对象都不共享。shared table/global 属于后来的 shared-everything-threads 提案，不在范围内。
- wasi-threads 只是 `wasi_thread_spawn` 一个函数。真正的阻塞、共享、同步全在 core 层，不在 WASI 层。
- 每 spawn 一个线程，把同一个 module 再实例化一次，导入同一块 shared memory，然后调 `wasi_thread_start(tid, arg)`。C 程序的 `__stack_pointer` 和 TLS 能正确工作，正是因为每个实例有自己的 globals。

## riscv64 现状

riscv64 后端**不支持 shared memory，因此也不支持 threads / wasi-threads**。x86_64 和 arm64 支持。

- `build.rs` 的 `emit_guard_pages_cfg` 只给 x86_64 / arm64 设 `sf_has_guard_pages`。原因是 `runtime/os/signal/` 下没有 riscv64 的 signal handler（没有 ucontext 解析和 PC 改写），guard page fault 无法变成 wasm trap。
- `SharedMemory`（`jit/shared_memory.rs`）依赖 guard-page 预留，挂在 `#[cfg(sf_has_guard_pages)]` 下，riscv64 上不编译。`SharedMemHandle`、`Import::shared_memory` 同样不存在。
- parser / validator 照常接受 shared memory；实例化时 riscv64 返回 unlinkable 错误 `shared memory is not supported on this target (no guard-page support)`（`jit/instantiate.rs`）。`memory.size` 的 shared lowering 在该 cfg 下同样返回这个错误，保证不会生成读 SharedMemory 头部的代码。
- riscv64 的 0xFE 原子指令只有 dispatch 桩，没有真实编码。

打开 riscv64 threads 的前提：先写 riscv64-Linux signal handler（guard page 现在只在 aarch64 和 x86_64 上启用），再做原子指令编码。目前只保证 `cargo check --target riscv64gc-unknown-linux-gnu -p wnjr-core` 通过，没有在 riscv64 上跑过测试。`wnjr-wasi`（以及依赖它的 `wnjr-cli`）在 riscv64 上**仍编译不过**：`wnjr-wasi/src/threads.rs` 无条件使用 `SharedMemHandle` 和 `Import::shared_memory`，这两个在该 target 上不存在。

## 三层模型

### 1. Module（编译产物）

不可变，`Arc`，`Send + Sync`。机器码、类型表、trap 元数据只读，多线程读同一份不写脏 cache line。`CodeBuffer` 和 `FunctionSpec` 已有 `unsafe impl Send + Sync`（module/entities.rs:148-149）；Module 实现 Clone（FunctionSpec clone 清空 native_code，每实例独立编译）。编译期静态断言见 wnjr-core/tests/wasi_threads.rs。

### 2. Instance / Store

只属于一个线程，`!Send`。`Rc<RefCell<..>>` 原样保留。globals、tables、DRC GC heap 都留在这里。DRC 的引用计数因此继续用非原子计数。单线程路径一行都不用改。

### 3. SharedMemory

唯一跨线程的对象。访问路径上没有锁。

- 基址固定。spec 要求 shared memory 必须声明 max。创建时一次性预留 `max * 64KiB` 加 guard 的虚拟地址空间，之后基址永远不变。JIT 在 `NativeContext` 里缓存的 mem0 基址一直有效。
- 长度用 `AtomicU64`。`memory.grow` 走冷路径：只在 grow 时用的锁，`mprotect` 打开新页，再 `Release` 写入新长度。读长度用 `Acquire`。越界检查靠 guard page，普通 load/store 连长度都不读。
- 宿主访问不做借用计数。现在 `vm/memory.rs` 每次访问都改 `Cell` 的 readers/writer 计数，放到跨线程就是热点 cache line，而且非原子。shared memory 只提供裸指针加 copy-in/copy-out。wasm 内存模型本来就允许数据竞争，宿主侧不用借用规则去守。

## 其余

### wait/notify

全局按地址索引的 parking lot：哈希表映射地址到等待队列，只有 `memory.atomic.wait`/`notify` 时才碰。Linux 上 `wait32` 直接对应 futex。`wait64` 对不上 futex 的字宽，走 parking lot。wasmtime 的 parking spot 是同一个做法。

**当前现状（th7 已提交）：** `wait32`/`wait64` 真实阻塞，`notify` 真实唤醒。实现为全局 WaitTable（HashMap + parking_lot Mutex/Condvar）。行为：
- value mismatch 立即返回 1
- 相等时：timeout=-1 挂起直到 notify，否则等待 timeout_ns 后返回 2（超时）或 0（被唤醒）
- `notify(addr, count)` 唤醒最多 count 个 waiter，返回实际唤醒数

### trap 表

现在是 `static mut TRAP_TABLE` 加 `AtomicBool` spinlock（`vm/jit/runtime/trap_signal.rs`）。signal handler 里拿 spinlock 是未定义行为：同线程重入会死锁。

改成一个 `AtomicPtr` 指向不可变的有序数组。注册时复制、插入、发布新指针。旧数组不释放，进程退出时一起走。注册只在编译时发生，泄漏量很小。handler 里只做 `Acquire` 读加二分查找，没有锁，async-signal-safe。

### 每线程状态

`trap_kind` 本来就在 `NativeContext` 里，靠 context register 访问，天然每线程一份。每个线程要有自己的执行栈和 stack guard，`GuardPageStack` 已是每 JitInstance 一份（vm/jit/instance.rs:52, 179-192）。

## 代价

每个线程多付一次实例化：重跑 globals、tables、data segment 初始化。shared memory 的 data segment 只初始化一次。

当前 JIT 实现为每实例独立编译（FunctionSpec::clone 清空 native_code / native_cache，Instance::from_module 按需触发 ensure_module_compiled）。ThreadsConfig 缓存 Module（解析+校验只做一次），但 worker 的 Instance::from_module 仍会独立编译。spawn 耗时从 ~26ms 压到 <1ms（解析+校验是主要成本）。编译共享属于后续优化，需审 CompiledNativeModule 的 Send+Sync。和每条访存都付的锁相比，当前的每线程编译仍是一次性成本。

## wasi-threads 宿主侧（th10）

宿主只提供一样东西：`("wasi" "thread-spawn") (param i32) -> (result i32)`。实现见
`wnjr-wasi/src/threads.rs`。

- 返回值必须是 tid ≥ 1。guest 用 `<= 0` 判失败，0 在规范里表示"没有线程"。
- 每次 spawn 在**新 OS 线程**里把同一个 module 再实例化一次，导入同一块 shared
  memory，再调 `wasi_thread_start(tid, start_arg)`。globals、tables、TLS 是每实例
  一份，所以 `__stack_pointer` 和 `pthread_self` 天然正确。
- module 必须 import 它的 memory（wasi-sdk 用 `--import-memory --shared-memory`）。
  自己定义 memory 的 module 无法跨实例共享，直接拒绝并给出该用的链接参数。
- worker 的失败（实例化失败、缺 export、trap）记进 `ThreadRegistry`，宿主可以读到
  并断言；不许在子线程里 panic 被吞掉。

### proc_exit 与"等所有线程结束"的说法

之前这里写过"main 返回后进程要等所有线程结束才退出"。核过官方 wasi-threads
testsuite 之后这条是错的，按 testsuite 为准：

- 从 `_start` 返回等价于 `proc_exit(0)`，**不**等待其它线程。`exit_main_busy` /
  `return_main_busy` 用例专门造一个死循环的 worker，要求 main 返回后整个进程立即
  退出，把它们一起带走。
- 任意线程调 `proc_exit` 都终止整个程序，包括正阻塞在 `wait32` 或 `poll_oneoff`
  里的线程（`exit_nonmain_block` / `exit_nonmain_wasi` 就是测这个）。
- 因此 CLI 在退出前**不** join worker：会挂死。`ThreadRegistry::join_all` 只给宿主
  程序自己用（跑完一轮、要收集失败时）。

### 已知限制

每线程实例化会重跑 data segment。wasi-sdk 的多线程构建用 passive segment 加
`__wasm_init_memory` 守卫，只初始化一次，所以真实的 pthread 程序没问题；一个用
active data segment 写共享内存的手写 module，第二个实例会把第一个实例写的东西盖掉。

## 明确不做

shared-everything-threads。如果以后要跨线程共享 GC 对象，DRC 计数得改成原子操作，或者换回收方案。那是另一份设计。

## 实现顺序

1. Module 的 `Send + Sync`（th1），trap 表改 RCU（th2）。这两件不依赖 shared memory，可以先做。
2. parser/validator 接受 shared memory，要求有 max（th3）。
3. `SharedMemory`：预留地址空间、原子长度、裸指针访问（th4）。
4. `0xFE` atomics 解码、验证、自然对齐 trap（th5），三个后端的 lowering（th6）。
5. wait/notify（th7）。
6. 每线程实例化（th8），然后 `wasi_thread_spawn`（th9）。
7. 打开 spectest 的 threads proposal 测试和 wasi-testsuite 的 threads 用例（th10）。
