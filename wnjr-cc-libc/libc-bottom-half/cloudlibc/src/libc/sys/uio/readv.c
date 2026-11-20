// Copyright (c) 2015-2016 Nuxi, https://nuxi.nl/
//
// SPDX-License-Identifier: BSD-2-Clause

#include <sys/uio.h>

#include <assert.h>
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <unistd.h>
#include <wasi/api.h>

#ifndef __wasip1__
#include <wasi/descriptor_table.h>
#include <common/errors.h>
#endif

// struct iovec and __wasi_iovec_t used to be the same bytes, and this file
// reinterpreted one as the other, with asserts above the cast that said so. They
// are not the same bytes in this data model: a C pointer is eight bytes, the low
// half an address in the memory and the high half a collector tag, while the ABI
// spells an address as four. The two structures therefore have different layouts
// and the translation below is element by element, truncating the address the way
// every call into the ABI here does.

ssize_t readv(int fildes, const struct iovec *iov, int iovcnt) {
  if (iovcnt < 0 || IOV_MAX < iovcnt) {
    errno = EINVAL;
    return -1;
  }

#if defined(__wasip1__)
  ssize_t total = 0;
  while (iovcnt > 0) {
    enum { CHUNK = 8 };
    __wasi_iovec_t chunk[CHUNK];
    int n = iovcnt < CHUNK ? iovcnt : CHUNK;
    size_t want = 0;
    for (int i = 0; i < n; i++) {
      chunk[i].buf = (uint32_t)(uintptr_t)iov[i].iov_base;
      chunk[i].buf_len = iov[i].iov_len;
      want += iov[i].iov_len;
    }
    size_t bytes_read = 0;
    __wasi_errno_t error = __wasi_fd_read(fildes, chunk, n, &bytes_read);
    if (error != 0) {
      errno = error;
      return -1;
    }
    total += bytes_read;
    // A short read ends the loop, and a full one leaves the buffer as it is:
    // either way the descriptors are consumed in order.
    if (bytes_read < want)
      break;
    iov += n;
    iovcnt -= n;
  }
  return total;
#elif defined(__wasip2__) || defined(__wasip3__) || defined(__webp2__)
  while (iovcnt) {
    if (iov->iov_len != 0)
      return read(fildes, iov->iov_base, iov->iov_len);
    iovcnt--;
    iov++;
  }
  return read(fildes, NULL, 0);
#else
# error "Unsupported WASI version"
#endif
}
