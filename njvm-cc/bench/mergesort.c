// Sort an array with its own generator and its own sort. Neither rand() nor
// qsort() is used: a libc difference would move the checksum and the comparison
// would be measuring the wrong thing.
#include <stdio.h>

#define N 1000000

static unsigned x = 123456789u;
static unsigned next(void) {
  x ^= x << 13;
  x ^= x >> 17;
  x ^= x << 5;
  return x;
}

static int t[N];
static int tmp[N];

static void merge(int lo, int mid, int hi) {
  int i = lo, j = mid, k = lo;
  while (i < mid && j < hi) {
    if (t[i] <= t[j])
      tmp[k++] = t[i++];
    else
      tmp[k++] = t[j++];
  }
  while (i < mid)
    tmp[k++] = t[i++];
  while (j < hi)
    tmp[k++] = t[j++];
  for (i = lo; i < hi; i++)
    t[i] = tmp[i];
}

static void sort(int lo, int hi) {
  if (hi - lo < 2)
    return;
  int mid = lo + (hi - lo) / 2;
  sort(lo, mid);
  sort(mid, hi);
  merge(lo, mid, hi);
}

int main(void) {
  for (int i = 0; i < N; i++)
    t[i] = (int)(next() >> 8) - 8388608;

  sort(0, N);

  unsigned long long h = 1469598103934665603ull;
  for (int i = 0; i < N; i++) {
    h ^= (unsigned long long)(unsigned)t[i];
    h *= 1099511628211ull;
  }
  printf("checksum %lld\n", (long long)(h & 0x7fffffffffffffffll));
  return 0;
}
