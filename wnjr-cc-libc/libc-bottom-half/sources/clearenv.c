// There is nothing to free and nothing to reallocate: the environment here is a
// snapshot the runtime was started with, turned into an array once by
// libc-environ. Emptying it means pointing the array at its own terminator, and
// that is the whole effect C promises.
#include <stddef.h>
#include <stdlib.h>

extern char **__wasilibc_get_environ(void);

int clearenv(void) {
  __wasilibc_get_environ()[0] = NULL;
  return 0;
}
