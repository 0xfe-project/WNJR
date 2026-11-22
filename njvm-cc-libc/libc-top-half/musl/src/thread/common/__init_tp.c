// This runtime has one execution context per instance, and wasm32's thread
// pointer is a _Thread_local struct pthread (arch/wasm32/pthread_arch.h), so that
// context has its own copy of the structure and __init_tp only has to fill in
// what one thread needs.
//
// Upstream keeps this function in env/__init_tls.c, next to the static-TLS
// setup, which reads the stack pointer out of a wasm global with inline asm,
// builds the TLS image and registers the thread with a kernel. There is no
// libcall thread context here and no kernel, and the compiler owns the layout of
// the TLS block (a _Thread_local variable is an ordinary object in the data
// segment, so nothing has to be copied).
#include "pthread_impl.h"

int __init_tp(void *p) {
  pthread_t td = p;
  // __pthread_self compares this pointer to decide whether the first use has
  // happened yet, so it is what makes the initialization below run once.
  td->self = td;
  // A tid of zero is musl's marker for "no thread holds this lock". With one
  // thread nothing can contend, so no lock is ever taken and no call can block.
  td->tid = 0;
  td->detach_state = DT_DETACHED;
  // CURRENT_LOCALE dereferences this, and the C locale is the only one here.
  td->locale = &libc.global_locale;
  return 0;
}
