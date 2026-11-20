// The GNU __sync builtins, which are what musl's atomic_arch.h uses to build a
// lock. Each one is a real NJVM-IR atomic op, so the memory is the only place
// the value ever lives.
//
// Single-threaded, but every one of these is checked against the host compiler,
// so the values are the actual results of the operations rather than a guess.

int gi = 7;
long gl = 100;
char gc = 3;
short gs = -4;

// __sync_val_compare_and_swap returns what the location held before.
int cas_int(int expected, int desired) {
  return __sync_val_compare_and_swap(&gi, expected, desired);
}

int cas_bool(int expected, int desired) {
  return __sync_bool_compare_and_swap(&gi, expected, desired);
}

// The driver calls every export as int f(int, ...), so the 64-bit cases are
// wrapped in int signatures.
int cas_long(int expected, int desired) {
  return (int)__sync_val_compare_and_swap(&gl, (long)expected, (long)desired);
}

// 1-byte and 2-byte atomics: the widths that are not a full word.
int cas_char(int expected, int desired) {
  return (int)__sync_val_compare_and_swap(&gc, (char)expected, (char)desired);
}

int cas_short(int expected, int desired) {
  return (int)__sync_val_compare_and_swap(&gs, (short)expected, (short)desired);
}

int fetch_add(int v) { return __sync_fetch_and_add(&gi, v); }
int fetch_sub(int v) { return __sync_fetch_and_sub(&gi, v); }
int fetch_and(int v) { return __sync_fetch_and_and(&gi, v); }
int fetch_or(int v) { return __sync_fetch_and_or(&gi, v); }
int fetch_xor(int v) { return __sync_fetch_and_xor(&gi, v); }
int fetch_add_long(int v) { return (int)__sync_fetch_and_add(&gl, (long)v); }

int test_and_set(int v) { return __sync_lock_test_and_set(&gi, v); }

int release_and_read(void) {
  __sync_synchronize();
  __sync_lock_release(&gi);
  return gi;
}

// A lock built the way musl builds one, taken and released in a loop, with a
// counter protected by it. The counter must reach exactly the loop count.
int lock = 0;
int protected_count = 0;

static void lock_acquire(void) {
  while (__sync_val_compare_and_swap(&lock, 0, 1) != 0)
    ;
}

static void lock_release(void) { __sync_lock_release(&lock); }

int guarded_increment(int times) {
  protected_count = 0;
  for (int i = 0; i < times; i++) {
    lock_acquire();
    protected_count = protected_count + 1;
    lock_release();
  }
  return protected_count;
}

// Two counters driven by read-modify-writes: the returned old values sum to the
// arithmetic series, which only holds if each add is a read-modify-write.
int series(int n) {
  int a = 0;
  long b = 0;
  int sum = 0;
  for (int i = 0; i < n; i++) {
    sum += __sync_fetch_and_add(&a, i);
    __sync_fetch_and_add(&b, 2);
  }
  return sum + (int)b;
}

int long_state(void) { return (int)gl; }

int reset(void) {
  gi = 7;
  gl = 100;
  gc = 3;
  gs = -4;
  lock = 0;
  return gi;
}
