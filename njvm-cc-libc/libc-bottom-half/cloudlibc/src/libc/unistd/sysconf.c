// musl answers sysconf by asking the kernel, through sys/sysinfo.h and a pile of
// _SC_* cases that describe a Linux process. This runtime has no kernel to ask,
// so the answers are the constants it actually runs with: one processor, a page
// per 64 KiB of linear memory (what dlmalloc and stdio assume), and time in
// nanoseconds, so a tick is a microsecond.
#include <unistd.h>
#include <errno.h>

long sysconf(int name) {
  switch (name) {
  case _SC_PAGESIZE:
    return 65536;
  case _SC_NPROCESSORS_CONF:
  case _SC_NPROCESSORS_ONLN:
    return 1;
  case _SC_CLK_TCK:
    return 1000000;
  case _SC_OPEN_MAX:
    return 1024;
  case _SC_VERSION:
    return 200809L;
  case _SC_ARG_MAX:
    return 4096;
  case _SC_CHILD_MAX:
    return 0;
  default:
    errno = EINVAL;
    return -1;
  }
}
