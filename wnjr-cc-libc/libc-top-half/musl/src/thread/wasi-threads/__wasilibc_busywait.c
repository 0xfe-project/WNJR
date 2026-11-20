// The upstream file defines __wasilibc_use_busy_futex with inline asm that
// declares a wasm global, and reads it before every wait. Busy-waiting exists to
// avoid trapping into the host for a wait that is about to end, and this
// compiler emits no asm, so the flag is a plain variable that stays false: every
// wait goes to the host.
#include <stdint.h>
#include <errno.h>

static int32_t __wasilibc_use_busy_futex;

void __wasilibc_enable_futex_busywait_on_current_thread(void) {
  __wasilibc_use_busy_futex = 1;
}

int __wasilibc_futex_wait_atomic_wait(volatile void *addr, int val, int64_t max_wait_ns);

int __wasilibc_futex_wait_maybe_busy(volatile void *addr, int val, int64_t max_wait_ns) {
  if (!__wasilibc_use_busy_futex)
    return __wasilibc_futex_wait_atomic_wait(addr, val, max_wait_ns);
  // The busy path spins on the word until it changes or the deadline passes.
  // Without clock_gettime this is a bounded spin with no deadline.
  while (*(volatile int *)addr == val)
    ;
  return 0;
}
