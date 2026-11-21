// Count the solutions of the n queens problem by backtracking over bit masks.
// Integer, no library call, and the count is a known number.
#include <stdio.h>

#define N 14

static long long count;
static unsigned full;

static void place(unsigned cols, unsigned diag1, unsigned diag2) {
  if (cols == full) {
    count++;
    return;
  }
  unsigned free = full & ~(cols | diag1 | diag2);
  while (free) {
    unsigned bit = free & (~free + 1u);
    free -= bit;
    place(cols | bit, (diag1 | bit) << 1, (diag2 | bit) >> 1);
  }
}

int main(void) {
  full = (1u << N) - 1u;
  place(0, 0, 0);
  printf("checksum %lld\n", count);
  return 0;
}
