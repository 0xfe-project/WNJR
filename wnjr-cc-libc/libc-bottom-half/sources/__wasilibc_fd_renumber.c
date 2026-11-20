#include <errno.h>
#include <unistd.h>
#include <wasi/api.h>
#include <wasi/libc.h>

#if !defined(__wasip1__)
#include <wasi/descriptor_table.h>
#include <wasi/file_utils.h>
#endif

#ifdef __wasip1__
int __wasilibc_fd_renumber(int fd, int newfd) {
  // Scan the preopen fds before making any changes.
  __wasilibc_populate_preopens();

  __wasi_errno_t error = __wasi_fd_renumber(fd, newfd);
  if (error != 0) {
    errno = error;
    return -1;
  }
  return 0;
}
#endif

int close(int fd) {
#ifndef __webp2__
  // Scan the preopen fds before making any changes.
  __wasilibc_populate_preopens();
#endif

#if defined(__wasip1__)
  __wasi_errno_t error = __wasi_fd_close(fd);
  if (error != 0) {
    errno = error;
    return -1;
  }
#elif defined(__wasip2__) || defined(__wasip3__) || defined(__webp2__)
  if (descriptor_table_remove(fd) < 0)
    return -1;
#else
#error "Unsupported WASI version"
#endif // __wasip2__

  return 0;
}

/* The weak placeholder that upstream carries here is for programs that link
 * without preopens.c: this tree always has it, and this frontend reports two
 * definitions of one name rather than letting the linker pick the strong one. */
