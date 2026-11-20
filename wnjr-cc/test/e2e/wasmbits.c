// The wasm builtins a C library needs and no host compiler has: memory.size,
// memory.grow, the two atomic waits and the notify. clang on the host knows none
// of them, so this case carries expected results instead of a native build.
//
// The values are the ones the wasm specification defines, not a snapshot of
// this engine: a wait whose expected value does not match returns 1 at once, a
// wait nobody wakes returns 2 when its timeout expires, and a notify with no
// waiter returns 0.

int flag = 0;
long wide = 0;

// Pages are 64 KiB, and the module starts with at least one of them.
int pages_positive(void) { return __builtin_wasm_memory_size(0) > 0; }

// Growing by one page yields the previous count and adds exactly one page.
// The two halves are summed so a failure of either shows up in the value.
int grow_one(void) {
  long before = __builtin_wasm_memory_size(0);
  long old = __builtin_wasm_memory_grow(0, 1);
  long after = __builtin_wasm_memory_size(0);
  return (old == before) + (after == before + 1);
}

// Growing by zero is allowed and changes nothing; the spec does not promise it
// reports success, so only the page count is checked.
int grow_zero(void) {
  long before = __builtin_wasm_memory_size(0);
  __builtin_wasm_memory_grow(0, 0);
  return __builtin_wasm_memory_size(0) == before;
}

// A wait on a value that is not the expected one returns 1 immediately.
int wait32_mismatch(void) { return __builtin_wasm_memory_atomic_wait32(&flag, 1, 0); }

// A wait on the value that is there and nobody to wake it runs out of time: 2.
int wait32_timeout(void) { return __builtin_wasm_memory_atomic_wait32(&flag, 0, 1000000); }

int wait64_mismatch(void) { return __builtin_wasm_memory_atomic_wait64(&wide, 1, 0); }

int wait64_timeout(void) { return __builtin_wasm_memory_atomic_wait64(&wide, 0, 1000000); }

// Nothing is waiting, so nothing is woken.
int notify_none(void) { return __builtin_wasm_memory_atomic_notify(&flag, 1); }

// A page of memory is reachable: the last byte of the first page is writable.
int page_writable(void) {
  char *p = (char *)0;
  p[65535] = 7;
  p[65535] = 0;
  return 1;
}
