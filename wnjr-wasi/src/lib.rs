//! WASI preview1 support for wnjr-core.
//!
//! Each import set owns its WASI context. Create separate import sets for
//! isolated instances; cloning an import intentionally shares its context.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::string::{String, ToString};
use wnjr_core::value_type::ValueType;
use wnjr_core::{FunctionType, WasmError};

use wnjr_core::Import;

pub(crate) mod preview1;

pub mod threads;

pub use threads::{
    imported_memories, imports_thread_spawn, memory_imports_for, thread_spawn_import, MemoryImport,
    ThreadRegistry, ThreadsConfig, THREAD_START_EXPORT, WASI_THREADS_MODULE,
};

pub const WASI_SNAPSHOT_PREVIEW1: &str = "wasi_snapshot_preview1";
pub const WASI_UNSTABLE: &str = "wasi_unstable";

// ---------------------------------------------------------------------------
// WasiCtx — runtime state for a WASI instance
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct PreopenDir {
    pub guest_path: String,
    pub host_path: PathBuf,
}

enum FdEntry {
    Dir {
        host_path: PathBuf,
        rights_base: u64,
        rights_inh: u64,
    },
    File {
        file: std::fs::File,
        host_path: PathBuf,
        rights_base: u64,
        rights_inh: u64,
        fdflags: u16,
    },
}

// FdEntry contains File which is not Debug, provide manual impl
impl std::fmt::Debug for FdEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FdEntry::Dir { host_path, .. } => {
                f.debug_struct("Dir").field("host_path", host_path).finish()
            }
            FdEntry::File { host_path, .. } => f
                .debug_struct("File")
                .field("host_path", host_path)
                .finish(),
        }
    }
}

pub struct WasiCtx {
    args: std::vec::Vec<String>,
    env: std::vec::Vec<(String, String)>,
    preopens: std::vec::Vec<PreopenDir>,
    next_fd: i32,
    fds: HashMap<i32, FdEntry>,
    closed_preopens: HashSet<i32>,
    closed_stdio: HashSet<i32>,
    /// State for random_get: 0 until the first call seeds it. It has to outlive
    /// a call — see preview1.rs:random_get.
    rng: u64,
}

impl WasiCtx {
    fn new(
        args: std::vec::Vec<String>,
        env: std::vec::Vec<(String, String)>,
        preopens: std::vec::Vec<PreopenDir>,
    ) -> Self {
        let next_fd = 3 + preopens.len() as i32;
        Self {
            args,
            env,
            preopens,
            next_fd,
            fds: HashMap::new(),
            closed_preopens: HashSet::new(),
            closed_stdio: HashSet::new(),
            rng: 0,
        }
    }

    fn alloc_fd(&mut self, entry: FdEntry) -> i32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        self.fds.insert(fd, entry);
        fd
    }
}

// ---------------------------------------------------------------------------
// WasiContextBuilder
// ---------------------------------------------------------------------------

/// Copyable recipe for a WasiCtx: workers build their own from it.
#[derive(Clone)]
pub struct WasiContextBuilder {
    args: std::vec::Vec<String>,
    env: std::vec::Vec<(String, String)>,
    preopens: std::vec::Vec<PreopenDir>,
}

impl WasiContextBuilder {
    pub fn new() -> Self {
        Self {
            args: std::vec!["program".into()],
            env: std::vec![],
            preopens: std::vec![],
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.args = args.into_iter().map(|s| s.as_ref().to_string()).collect();
        self
    }

    pub fn env<K: AsRef<str>, V: AsRef<str>>(mut self, k: K, v: V) -> Self {
        self.env
            .push((k.as_ref().to_string(), v.as_ref().to_string()));
        self
    }

    pub fn inherit_env(mut self) -> Self {
        for (k, v) in std::env::vars() {
            self.env.push((k, v));
        }
        self
    }

    pub fn preopen_dir<P: Into<PathBuf>, S: AsRef<str>>(mut self, guest: S, host: P) -> Self {
        self.preopens.push(PreopenDir {
            guest_path: guest.as_ref().to_string(),
            host_path: host.into(),
        });
        self
    }

    pub fn build(self) -> WasiCtx {
        WasiCtx::new(self.args, self.env, self.preopens)
    }
}

impl Default for WasiContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WasiCtx {
    fn with_ctx<R>(&self, f: impl FnOnce(&WasiCtx) -> R) -> R {
        f(self)
    }

    fn with_ctx_mut<R>(&mut self, f: impl FnOnce(&mut WasiCtx) -> R) -> R {
        f(self)
    }
}

// ---------------------------------------------------------------------------
// WASI Import generation
// ---------------------------------------------------------------------------

/// Generate WASI imports for a given module namespace.
fn wasi_imports_for(module: &str, context: &Rc<RefCell<WasiCtx>>) -> std::vec::Vec<Import> {
    macro_rules! wasi {
        ($f:ident, [$($param:ident),*], [$($result:ident),*]) => {{
            let context = Rc::clone(context);
            Import::func_typed(module, stringify!($f), move |caller, args, results| {
                let mut context = context
                    .try_borrow_mut()
                    .map_err(|_| WasmError::trap("WASI context is already in use"))?;
                context.$f(caller, args, results)
            }, FunctionType::new(
                std::vec![$(ValueType::$param),*],
                std::vec![$(ValueType::$result),*],
            ))
        }};
    }

    std::vec![
        wasi!(args_sizes_get, [I32, I32], [I32]),
        wasi!(args_get, [I32, I32], [I32]),
        wasi!(environ_sizes_get, [I32, I32], [I32]),
        wasi!(environ_get, [I32, I32], [I32]),
        wasi!(fd_write, [I32, I32, I32, I32], [I32]),
        wasi!(fd_read, [I32, I32, I32, I32], [I32]),
        wasi!(fd_close, [I32], [I32]),
        wasi!(fd_seek, [I32, I64, I32, I32], [I32]),
        wasi!(fd_tell, [I32, I32], [I32]),
        wasi!(fd_fdstat_get, [I32, I32], [I32]),
        wasi!(fd_fdstat_set_flags, [I32, I32], [I32]),
        wasi!(fd_fdstat_set_rights, [I32, I64, I64], [I32]),
        wasi!(fd_prestat_get, [I32, I32], [I32]),
        wasi!(fd_prestat_dir_name, [I32, I32, I32], [I32]),
        wasi!(fd_filestat_get, [I32, I32], [I32]),
        wasi!(fd_filestat_set_size, [I32, I64], [I32]),
        wasi!(fd_filestat_set_times, [I32, I64, I64, I32], [I32]),
        wasi!(fd_sync, [I32], [I32]),
        wasi!(fd_datasync, [I32], [I32]),
        wasi!(fd_renumber, [I32, I32], [I32]),
        wasi!(fd_readdir, [I32, I32, I32, I64, I32], [I32]),
        wasi!(fd_pread, [I32, I32, I32, I64, I32], [I32]),
        wasi!(fd_pwrite, [I32, I32, I32, I64, I32], [I32]),
        wasi!(fd_allocate, [I32, I64, I64], [I32]),
        wasi!(fd_advise, [I32, I64, I64, I32], [I32]),
        wasi!(clock_time_get, [I32, I64, I32], [I32]),
        wasi!(clock_res_get, [I32, I32], [I32]),
        wasi!(random_get, [I32, I32], [I32]),
        wasi!(proc_exit, [I32], []),
        wasi!(sched_yield, [], [I32]),
        wasi!(sock_shutdown, [I32, I32], [I32]),
        wasi!(poll_oneoff, [I32, I32, I32, I32], [I32]),
        wasi!(path_create_directory, [I32, I32, I32], [I32]),
        wasi!(path_filestat_get, [I32, I32, I32, I32, I32], [I32]),
        wasi!(
            path_filestat_set_times,
            [I32, I32, I32, I32, I64, I64, I32],
            [I32]
        ),
        wasi!(
            path_open,
            [I32, I32, I32, I32, I32, I64, I64, I32, I32],
            [I32]
        ),
        wasi!(path_readlink, [I32, I32, I32, I32, I32, I32], [I32]),
        wasi!(path_remove_directory, [I32, I32, I32], [I32]),
        wasi!(path_unlink_file, [I32, I32, I32], [I32]),
        wasi!(path_rename, [I32, I32, I32, I32, I32, I32], [I32]),
        wasi!(path_link, [I32, I32, I32, I32, I32, I32, I32], [I32]),
        wasi!(path_symlink, [I32, I32, I32, I32, I32], [I32]),
    ]
}

/// Bind preview1 and legacy WASI imports to an owned context.
///
/// Call this separately for each isolated instance. Cloning or reusing these
/// imports intentionally shares arguments, environment, descriptors and their
/// state. No thread-local setup is required. Resources are released when the
/// last instance or import retaining this context is dropped.
pub fn wasi_imports(context: WasiCtx) -> std::vec::Vec<Import> {
    wasi_imports_with_config(context, None)
}

/// Like `wasi_imports`, but with `wasi` `thread-spawn` when `threads` is set.
///
/// `thread-spawn` is registered under the `wasi` namespace, which is the one
/// wasi-threads specifies and the one wasi-libc imports it from.
pub fn wasi_imports_with_config(
    context: WasiCtx,
    threads: Option<ThreadsConfig>,
) -> std::vec::Vec<Import> {
    let context = Rc::new(RefCell::new(context));
    let mut imports = wasi_imports_for(WASI_SNAPSHOT_PREVIEW1, &context);
    imports.extend(wasi_imports_for(WASI_UNSTABLE, &context));
    if let Some(threads) = &threads {
        imports.push(threads::thread_spawn_import(threads));
    }
    imports
}
