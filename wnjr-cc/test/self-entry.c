// The entry point of the wnjr-cc that runs on WNJR.
//
// libc has its own __main_void, but it works by the compiler renaming main to
// __main_argc_argv, and this front end does not rename anything. So the same
// twenty lines live here and call main directly: read what the host handed us
// through args_get, build the argv vector, go. malloc and calloc are the real
// ones — a compiler that cannot allocate its own command line is not a compiler.
#include <stdio.h>
#include <stdlib.h>
#include <wasi/api.h>

int main(int argc, char **argv);

int bootstrap_main(void) {
  size_t argc = 0;
  size_t buf_size = 0;
  if (__wasi_args_sizes_get(&argc, &buf_size) != 0) return 70;

  char *buf = malloc(buf_size ? buf_size : 1);
  // args_get hands back wasm32 pointers — four bytes each, packed — because that
  // is the shape of an address on the other side of the import. A pointer here
  // is eight bytes and this array would be interleaved garbage if the host
  // filled it directly: that costs nothing to get wrong and a while to see.
  // So collect them at their own width and widen after.
  uint32_t *ptrs = calloc(argc + 1, sizeof(uint32_t));
  char **argv = calloc(argc + 1, sizeof(char *));
  if (buf == 0 || ptrs == 0 || argv == 0) return 70;
  if (__wasi_args_get((uint8_t **)ptrs, (uint8_t *)buf) != 0) return 70;
  for (size_t i = 0; i < argc; i++) argv[i] = (char *)(unsigned long)ptrs[i];

  int r = main((int)argc, argv);
  fprintf(stderr, "BOOT main returned %d\n", r);
  return r;
}
