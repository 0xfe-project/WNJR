//! wasi-threads: the 'wasi' 'thread-spawn' host import.
//!
//! The threads proposal is core wasm (shared memory, atomics, wait/notify);
//! wasi-threads adds exactly one import. Its model is "instance per thread":
//! the host re-instantiates the same module on a new OS thread, hands it the
//! *same* shared memory, and calls the module's exported
//! wasi_thread_start(tid, start_arg) there.
//!
//! Consequences that drive this implementation:
//!
//! * The module must import its memory (wasi-sdk links threaded modules with
//!   --import-memory --shared-memory). A module that defines its own memory
//!   cannot share it across instances, so ThreadsConfig::new rejects it.
//! * Each worker gets fresh instances of everything else - globals, tables,
//!   the WASI context - which is what makes pthread_self and thread-local
//!   storage work through an ordinary global.
//! * proc_exit from *any* thread ends the whole program. A worker that exits
//!   goes through the registry's exit hook, which by default is
//!   std::process::exit.

use std::string::{String, ToString};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use wnjr_core::value_type::ValueType;
use wnjr_core::{
    Engine, FunctionType, Import, Instance, Limits, Module, SharedMemHandle, Value, WasmError,
};

use crate::{wasi_imports_with_config, WasiContextBuilder};

/// errno for a malformed call or an unsupported module.
const ERRNO_INVAL: i32 = 28;
/// errno for "the host could not create the thread".
const ERRNO_AGAIN: i32 = 6;

/// The import module wasi-threads puts thread-spawn in.
pub const WASI_THREADS_MODULE: &str = "wasi";
/// The export every wasi-threads module has to provide.
pub const THREAD_START_EXPORT: &str = "wasi_thread_start";

/// One memory imported by a module, resolved from its import section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryImport {
    /// Import namespace (env, foo, ...).
    pub module: String,
    /// Import name.
    pub name: String,
    /// Declared initial pages.
    pub min: u32,
    /// Declared maximum pages, when present.
    pub max: Option<u32>,
    /// Whether the declared limits carried the shared flag.
    pub shared: bool,
    /// Whether the memory is a 64-bit one (memory64).
    pub is64: bool,
}

/// Shared state of one wasi-threads program.
///
/// Hand the same registry to every worker so tids stay unique, failures come
/// back to the host, and proc_exit can end the program.
pub struct ThreadRegistry {
    next_tid: AtomicI32,
    handles: Mutex<Vec<(i32, std::thread::JoinHandle<()>)>>,
    errors: Mutex<Vec<String>>,
    exit: Arc<dyn Fn(i32) + Send + Sync>,
}

impl ThreadRegistry {
    /// A registry whose workers end the process through std::process::exit
    /// when they call proc_exit, as the wasi-threads contract requires.
    pub fn new() -> Self {
        Self::with_exit_hook(|code| std::process::exit(code))
    }

    /// A registry with a custom exit hook, for embedders that must not end
    /// the host process (tests, servers hosting several programs).
    pub fn with_exit_hook<F>(hook: F) -> Self
    where
        F: Fn(i32) + Send + Sync + 'static,
    {
        Self {
            next_tid: AtomicI32::new(1),
            handles: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            exit: Arc::new(hook),
        }
    }

    /// Failures reported by workers so far: instantiation errors, missing
    /// exports and traps. Host code is expected to assert this is empty.
    pub fn errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .expect("thread registry poisoned")
            .clone()
    }

    /// Join every worker spawned through this registry and return the failures
    /// they reported.
    ///
    /// Workers that never return (an infinite guest loop, or one parked in
    /// wait32 forever) make this block, which is why the CLI - a program whose
    /// exit code *is* the answer - does not call it.
    pub fn join_all(&self) -> Vec<String> {
        let handles = {
            let mut handles = self.handles.lock().expect("thread registry poisoned");
            core::mem::take(&mut *handles)
        };
        for (tid, handle) in handles {
            if handle.join().is_err() {
                self.errors
                    .lock()
                    .expect("thread registry poisoned")
                    .push(format!("thread {tid}: panicked"));
            }
        }
        self.errors()
    }

    fn record(&self, message: String) {
        self.errors
            .lock()
            .expect("thread registry poisoned")
            .push(message);
    }
}

impl Default for ThreadRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything a worker thread needs to start: the module to instantiate, the
/// shared memory to import into it, and the WASI context to give it.
#[derive(Clone)]
pub struct ThreadsConfig {
    engine: Engine,
    module: Arc<Module>,
    shared_memory: SharedMemHandle,
    memory_imports: Arc<Vec<(String, String)>>,
    context: WasiContextBuilder,
    registry: Arc<ThreadRegistry>,
}

impl ThreadsConfig {
    /// Validate that module_bytes can run under wasi-threads and bind it to
    /// shared_memory.
    ///
    /// Fails when the module defines its own memory instead of importing it
    /// (nothing to share), or does not export wasi_thread_start.
    pub fn new(
        engine: Engine,
        module_bytes: Arc<Vec<u8>>,
        shared_memory: SharedMemHandle,
    ) -> Result<Self, WasmError> {
        let module = Arc::new(Module::new("wasi-threads", &module_bytes)?);
        Self::from_module(engine, module, &module_bytes, shared_memory)
    }

    /// Like [`Self::new`], for a module the embedder already parsed.
    ///
    /// Workers instantiate clones of module, and every clone shares its
    /// compiled code: when the embedder instantiates the same module first,
    /// no worker compiles anything. module_bytes must be the bytes module was
    /// parsed from; they are read for the import and export checks only.
    pub fn from_module(
        engine: Engine,
        module: Arc<Module>,
        module_bytes: &[u8],
        shared_memory: SharedMemHandle,
    ) -> Result<Self, WasmError> {
        let memories = imported_memories(module_bytes)?;
        let shared: Vec<(String, String)> = memories
            .iter()
            .filter(|memory| memory.shared)
            .map(|memory| (memory.module.clone(), memory.name.clone()))
            .collect();
        if shared.is_empty() {
            return Err(WasmError::invalid(
                "wasi-threads needs the module to import its shared memory \
                 (link it with --import-memory --shared-memory)",
            ));
        }
        if !exports_function(module_bytes, THREAD_START_EXPORT)? {
            return Err(WasmError::invalid(
                "module does not export wasi_thread_start, so no thread can start",
            ));
        }
        Ok(Self {
            engine,
            module,
            shared_memory,
            memory_imports: Arc::new(shared),
            context: WasiContextBuilder::new(),
            registry: Arc::new(ThreadRegistry::new()),
        })
    }

    /// Give every worker a WASI context built from context.
    pub fn with_context(mut self, context: WasiContextBuilder) -> Self {
        self.context = context;
        self
    }

    /// Use a shared registry, so several ThreadsConfig values (or a parent and
    /// its nested spawns) agree on tids, join handles and the exit hook.
    pub fn with_registry(mut self, registry: Arc<ThreadRegistry>) -> Self {
        self.registry = registry;
        self
    }

    /// The registry this config spawns through.
    pub fn registry(&self) -> &Arc<ThreadRegistry> {
        &self.registry
    }

    /// The shared memory every worker imports.
    pub fn shared_memory(&self) -> &SharedMemHandle {
        &self.shared_memory
    }

    /// Imports for one worker instance: the same shared memory under the names
    /// the module expects, a fresh WASI context, and thread-spawn again so a
    /// guest thread can spawn further threads.
    fn worker_imports(&self) -> Vec<Import> {
        let mut imports: Vec<Import> = self
            .memory_imports
            .iter()
            .map(|(module, name)| Import::shared_memory(module, name, self.shared_memory.clone()))
            .collect();
        imports.extend(wasi_imports_with_config(
            self.context.clone().build(),
            Some(self.clone()),
        ));
        imports
    }

    fn run_worker(&self, tid: i32, start_arg: i32) {
        let error = match self.start_worker(tid, start_arg) {
            Ok(()) => return,
            Err(error) => error,
        };
        match error.exit_code() {
            Some(code) => (self.registry.exit)(code),
            None => self.registry.record(format!("thread {tid}: {error}")),
        }
    }

    fn start_worker(&self, tid: i32, start_arg: i32) -> Result<(), WasmError> {
        let imports = self.worker_imports();
        let module = (*self.module).clone();
        let mut instance = Instance::from_module(&self.engine, module, &imports)?;
        instance.invoke(
            THREAD_START_EXPORT,
            &[Value::I32(tid), Value::I32(start_arg)],
        )?;
        Ok(())
    }
}

/// Build the 'wasi' 'thread-spawn' import for config.
pub fn thread_spawn_import(config: &ThreadsConfig) -> Import {
    let config = config.clone();
    let spawn = move |_caller: &mut wnjr_core::Caller<'_>,
                      args: &[Value],
                      results: &mut [Value]|
          -> Result<(), WasmError> {
        let start_arg = match args.first() {
            Some(Value::I32(value)) => *value,
            _ => {
                results[0] = Value::I32(-ERRNO_INVAL);
                return Ok(());
            }
        };
        // tids start at 1: the spec reserves 0 for "no thread", and guests
        // treat a non-positive return as an error.
        let tid = config.registry.next_tid.fetch_add(1, Ordering::SeqCst);
        let worker = config.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("wnjr-thread-{tid}"))
            .spawn(move || worker.run_worker(tid, start_arg));
        match spawned {
            Ok(handle) => {
                config
                    .registry
                    .handles
                    .lock()
                    .expect("thread registry poisoned")
                    .push((tid, handle));
                results[0] = Value::I32(tid);
            }
            Err(_) => results[0] = Value::I32(-ERRNO_AGAIN),
        }
        Ok(())
    };
    Import::func_typed(
        WASI_THREADS_MODULE,
        "thread-spawn",
        spawn,
        FunctionType::new(std::vec![ValueType::I32], std::vec![ValueType::I32]),
    )
}

// ---------------------------------------------------------------------------
// Module binary inspection
// ---------------------------------------------------------------------------
//
// wasi-threads has to know two things before it can start a thread: which
// imports are memories (so a worker binds the same shared memory under the
// same names) and whether wasi_thread_start is exported. Both live in sections
// the embedder never otherwise sees, and both are cheap to read.

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 8 }
    }

    fn u8(&mut self) -> Result<u8, WasmError> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| WasmError::malformed("unexpected end of module"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn u32_leb(&mut self) -> Result<u32, WasmError> {
        let mut value: u32 = 0;
        let mut shift = 0;
        loop {
            let byte = self.u8()?;
            if shift >= 32 {
                return Err(WasmError::malformed("leb128 integer too long"));
            }
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn name(&mut self) -> Result<String, WasmError> {
        let length = self.u32_leb()? as usize;
        let end = self
            .pos
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| WasmError::malformed("name runs past end of module"))?;
        let text = core::str::from_utf8(&self.bytes[self.pos..end])
            .map_err(|_| WasmError::malformed("import name is not utf-8"))?;
        self.pos = end;
        Ok(text.to_string())
    }

    fn limits(&mut self) -> Result<(u32, Option<u32>, bool, bool), WasmError> {
        let flags = self.u8()?;
        if flags & !0x07 != 0 {
            return Err(WasmError::malformed("unsupported limits flags"));
        }
        let shared = flags & 0x02 != 0;
        let is64 = flags & 0x04 != 0;
        let has_max = flags & 0x01 != 0;
        let min = self.u32_leb()?;
        let max = if has_max { Some(self.u32_leb()?) } else { None };
        if shared && max.is_none() {
            return Err(WasmError::malformed("shared memory must declare a maximum"));
        }
        Ok((min, max, shared, is64))
    }

    /// Skip to the end of the section that started before the current pos.
    fn skip_section(&mut self, section_len: usize) -> Result<usize, WasmError> {
        let end = self
            .pos
            .checked_add(section_len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| WasmError::malformed("section runs past end of module"))?;
        Ok(end)
    }
}

fn check_header(bytes: &[u8]) -> Result<(), WasmError> {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return Err(WasmError::malformed("not a wasm module"));
    }
    Ok(())
}

/// Every memory the module imports, in import order.
pub fn imported_memories(bytes: &[u8]) -> Result<Vec<MemoryImport>, WasmError> {
    check_header(bytes)?;
    let mut reader = Reader::new(bytes);
    let mut memories = Vec::new();
    while reader.pos < bytes.len() {
        let section_id = reader.u8()?;
        let section_len = reader.u32_leb()? as usize;
        let section_end = reader.skip_section(section_len)?;
        if section_id == 2 {
            let count = reader.u32_leb()?;
            for _ in 0..count {
                let module = reader.name()?;
                let name = reader.name()?;
                match reader.u8()? {
                    0x00 => {
                        reader.u32_leb()?;
                    }
                    0x01 => {
                        reader.u8()?;
                        reader.limits()?;
                    }
                    0x02 => {
                        let (min, max, shared, is64) = reader.limits()?;
                        memories.push(MemoryImport {
                            module,
                            name,
                            min,
                            max,
                            shared,
                            is64,
                        });
                    }
                    0x03 => {
                        reader.u8()?;
                        reader.u8()?;
                    }
                    0x04 => {
                        reader.u8()?;
                        reader.u32_leb()?;
                    }
                    _ => return Err(WasmError::malformed("unknown import kind")),
                }
            }
            return Ok(memories);
        }
        reader.pos = section_end;
    }
    Ok(memories)
}

/// Whether the module exports a function called name.
pub fn exports_function(bytes: &[u8], name: &str) -> Result<bool, WasmError> {
    check_header(bytes)?;
    let mut reader = Reader::new(bytes);
    while reader.pos < bytes.len() {
        let section_id = reader.u8()?;
        let section_len = reader.u32_leb()? as usize;
        let section_end = reader.skip_section(section_len)?;
        if section_id == 7 {
            let count = reader.u32_leb()?;
            for _ in 0..count {
                let export_name = reader.name()?;
                let kind = reader.u8()?;
                reader.u32_leb()?;
                if kind == 0x00 && export_name == name {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        reader.pos = section_end;
    }
    Ok(false)
}

/// Whether the module imports 'wasi' 'thread-spawn'.
pub fn imports_thread_spawn(bytes: &[u8]) -> Result<bool, WasmError> {
    check_header(bytes)?;
    let mut reader = Reader::new(bytes);
    while reader.pos < bytes.len() {
        let section_id = reader.u8()?;
        let section_len = reader.u32_leb()? as usize;
        let section_end = reader.skip_section(section_len)?;
        if section_id == 2 {
            let count = reader.u32_leb()?;
            for _ in 0..count {
                let module = reader.name()?;
                let name = reader.name()?;
                let kind = reader.u8()?;
                if kind == 0x00 && module == WASI_THREADS_MODULE && name == "thread-spawn" {
                    return Ok(true);
                }
                match kind {
                    0x00 => {
                        reader.u32_leb()?;
                    }
                    0x01 => {
                        reader.u8()?;
                        reader.limits()?;
                    }
                    0x02 => {
                        reader.limits()?;
                    }
                    0x03 => {
                        reader.u8()?;
                        reader.u8()?;
                    }
                    0x04 => {
                        reader.u8()?;
                        reader.u32_leb()?;
                    }
                    _ => return Err(WasmError::malformed("unknown import kind")),
                }
            }
            return Ok(false);
        }
        reader.pos = section_end;
    }
    Ok(false)
}

/// Provision host imports for every memory the module imports.
///
/// A shared memory becomes a SharedMemHandle-backed import and is returned, so
/// the caller can hand it to ThreadsConfig and give every worker the same
/// bytes. A plain memory becomes an ordinary host memory of the declared size.
pub fn memory_imports_for(
    bytes: &[u8],
) -> Result<(Vec<Import>, Option<SharedMemHandle>), WasmError> {
    let mut imports = Vec::new();
    let mut shared = None;
    for memory in imported_memories(bytes)? {
        if memory.shared {
            if memory.is64 {
                return Err(WasmError::invalid(
                    "shared memory64 is not supported; only wasm32 shared memories are",
                ));
            }
            let max = memory
                .max
                .ok_or_else(|| WasmError::malformed("shared memory must declare a maximum"))?;
            let limits = Limits::new_shared(memory.min as usize, max as usize)?;
            let handle = SharedMemHandle::new(limits)?;
            imports.push(Import::shared_memory(
                &memory.module,
                &memory.name,
                handle.clone(),
            ));
            if shared.is_some() {
                return Err(WasmError::invalid(
                    "only one shared memory import is supported per module",
                ));
            }
            shared = Some(handle);
        } else if memory.is64 {
            let limits = Limits::new_64(memory.min as usize, memory.max.map(|m| m as usize))?;
            imports.push(Import::memory_with_limits(
                &memory.module,
                &memory.name,
                limits,
            ));
        } else {
            let limits = Limits::new(memory.min as usize, memory.max.map(|m| m as usize))?;
            imports.push(Import::memory_with_limits(
                &memory.module,
                &memory.name,
                limits,
            ));
        }
    }
    Ok((imports, shared))
}
