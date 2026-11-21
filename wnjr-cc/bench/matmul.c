// Square double matrix product. Nothing here comes from a library and the
// accumulation order is the order written, so the checksum cannot differ between
// two compilers that are both correct.
#include <stdio.h>

#define N 512

static double a[N][N];
static double b[N][N];
static double c[N][N];

int main(void) {
  for (int i = 0; i < N; i++)
    for (int j = 0; j < N; j++) {
      a[i][j] = (double)((i * 7 + j * 13) % 100) / 4.0;
      b[i][j] = (double)((i * 11 + j * 3) % 100) / 8.0;
    }

  for (int i = 0; i < N; i++)
    for (int j = 0; j < N; j++) {
      double s = 0.0;
      for (int k = 0; k < N; k++)
        s += a[i][k] * b[k][j];
      c[i][j] = s;
    }

  double sum = 0.0;
  for (int i = 0; i < N; i++)
    for (int j = 0; j < N; j++)
      sum += c[i][j];

  long long v = (long long)(sum * 1000.0);
  printf("checksum %lld\n", v);
  return 0;
}
