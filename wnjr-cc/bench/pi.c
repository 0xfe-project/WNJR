// Lattice points inside a quarter circle, by counting. Pure integer, no
// library call, no floating point: the answer is exact and any disagreement
// between two compilers is a real disagreement.
#include <stdio.h>

#define R 24000

int main(void) {
  long long hits = 0;
  for (long long x = 0; x < R; x++)
    for (long long y = 0; y < R; y++) {
      if (x * x + y * y < (long long)R * R)
        hits++;
    }
  printf("checksum %lld\n", hits);
  return 0;
}
