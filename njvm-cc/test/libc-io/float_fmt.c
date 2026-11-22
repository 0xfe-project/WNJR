// "%g" and "%e" of a value below one.
//
// The decimal exponent is negative there, and printf derives it from a pointer
// difference inside its own digit buffer. That difference was zero-extended, so
// the exponent came out 1073741823 and the digits were then read from an address
// far outside the buffer: an out of bounds trap, not a wrong digit. The cause is
// a whole-number bug in the frontend and test/e2e/ptrdiff.c pins it down; this
// is the same defect seen from the outside, where a user would meet it.
#include <stdio.h>

int main(void) {
  printf("%.3g %.3e\n", 0.5, 0.5);
  printf("%g %g %g\n", 0.25, 0.125, 0.1);
  printf("%.3g %.3g %.3g\n", 0.0, 1.0, 1234.5);
  printf("%Le %Lg\n", (long double)0.5, (long double)0.25);
  return 0;
}
