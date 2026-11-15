# wasi-threads test suite (vendored)

Copied verbatim from https://github.com/WebAssembly/wasi-threads
`test/testsuite/` at commit `f8ea03cc0ffbde143415e945497de112f6f9fd53`
(2024-07-17).

Each `<name>.wat` is one case. `<name>.json` carries the exit code the case
must produce. Cases without a `.json` are expected to exit with 0: for those
(returning from `_start`), the suite relies on the wasi-threads rule that
returning from `_start` is the same as `proc_exit(0)`.

`wasi_threads_testsuite.rs` in this directory is the runner: it encodes each
`.wat`, runs it in the real `wnjr-cli` binary as a child process and asserts
the child's exit status. Running out of process is the point - half of these
cases check that `proc_exit` from one thread kills the others, which cannot be
observed from inside the process being killed.
