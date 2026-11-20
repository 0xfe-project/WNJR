// A compiler reads a source file and writes a module, so the libc has to carry a
// file all the way out and back before the bootstrap can run: fopen for writing,
// a buffered stream, fclose that flushes, and a reopen that sees what was
// written. printf alone never exercised any of that.
#include <stdio.h>
#include <string.h>

static char buf[64];

int main(void) {
  int fails = 0;
  FILE *f = fopen("probe-out.txt", "w");
  if (!f) {
    printf("fopen w failed\n");
    return 1;
  }
  if (fputs("hello, file\n", f) < 0) fails++;
  if (fwrite("second line\n", 1, 12, f) != 12) fails++;
  if (fclose(f) != 0) fails++;

  FILE *g = fopen("probe-out.txt", "r");
  if (!g) {
    printf("fopen r failed\n");
    return 1;
  }
  size_t n = fread(buf, 1, sizeof buf - 1, g);
  buf[n] = 0;
  fclose(g);

  if (n != 24) fails++;
  if (strcmp(buf, "hello, file\nsecond line\n") != 0) fails++;

  printf("read_back=%d\n", (int)n);
  printf("fails=%d\n", fails);
  return fails;
}
