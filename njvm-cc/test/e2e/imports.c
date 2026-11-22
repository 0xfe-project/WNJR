// Imported functions: a declaration carrying wasi's import attribute names the
// module and field of the host function that stands in for it, and a call to it
// reaches the host. The host side here is wasi's snapshot, which njvm-cli run
// provides. The spelling is wasi-libc's own (`__wasilibc_real.c`).
//
// Each case returns 1 only if the call really crossed into the host and did
// what its signature promises — the host has to write into our memory for
// any of these to be 1.

typedef unsigned int u32;
typedef unsigned long long u64;

int args_sizes_get(u32 *argc, u32 *size)
    __attribute__((__import_module__("wasi_snapshot_preview1"),
                   __import_name__("args_sizes_get")));

int random_get(void *buf, u32 len)
    __attribute__((__import_module__("wasi_snapshot_preview1"),
                   __import_name__("random_get")));

int clock_time_get(u32 id, u64 precision, u64 *time)
    __attribute__((__import_module__("wasi_snapshot_preview1"),
                   __import_name__("clock_time_get")));

int fd_write(u32 fd, void *iovs, u32 iovs_len, u32 *nwritten)
    __attribute__((__import_module__("wasi_snapshot_preview1"),
                   __import_name__("fd_write")));

struct iovec {
  u32 base;
  u32 len;
};

// The host writes both words; a wrong pointer or a wrong argument order leaves
// them at zero.
int args_sizes(void) {
  u32 argc = 0;
  u32 size = 0;
  int err = args_sizes_get(&argc, &size);
  return err == 0 && argc >= 1 && size > 0;
}

// Two calls must not return the same bytes.
int random_twice(void) {
  unsigned char a[16];
  unsigned char b[16];
  if (random_get(a, 16) != 0 || random_get(b, 16) != 0)
    return 0;
  for (int i = 0; i < 16; i++)
    if (a[i] != b[i])
      return 1;
  return 0;
}

// A 64-bit argument and a 64-bit result through an import: the clock only goes
// forward, and it starts at zero so a second reading is not smaller.
int clock_forward(void) {
  u64 t1 = 0;
  u64 t2 = 0;
  if (clock_time_get(0, 1, &t1) != 0 || clock_time_get(0, 1, &t2) != 0)
    return 0;
  return t1 > 0 && t2 >= t1;
}

// Writing a message of known length to stderr: the host reads our buffer
// through a struct in our memory and reports how much it wrote.
int write_message(void) {
  char msg[6];
  msg[0] = 'i';
  msg[1] = 'm';
  msg[2] = 'p';
  msg[3] = 'o';
  msg[4] = 'r';
  msg[5] = 't';
  struct iovec iov;
  iov.base = (u32)msg;
  iov.len = 6;
  u32 written = 0;
  int err = fd_write(2, &iov, 1, &written);
  return err == 0 && written == 6;
}

// An imported function has an address like any other, so a call through a
// function pointer reaches the same host function.
int via_pointer(void) {
  int (*f)(u32 *, u32 *) = args_sizes_get;
  u32 argc = 0;
  u32 size = 0;
  return f(&argc, &size) == 0 && argc >= 1 && size > 0;
}
