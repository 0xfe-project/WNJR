// What "..." hands the callee.
//
// C's default argument promotions (6.5.2.2p6) say a char travels through an
// int and a float through a double, because the callee has no parameter to tell
// it what the argument was. A caller that writes only the argument's own width
// leaves the rest of the slot as it found it, and then the answer depends on
// what was stored there before: "-1" came out 255 when the three bytes above
// the one written happened to be zero, and 5b90ff when they did not.
#include <stdio.h>

int main(void) {
  char c = -1;
  unsigned char uc = 200;
  short s = -1234;
  unsigned short us = 65535;
  float f = 0.5f;
  _Bool b = 3;
  long l = -1234567890123L;

  // The promotions, with no cast in sight.
  printf("%d %d\n", c, uc);
  printf("%d %d\n", s, us);
  printf("%c%c\n", 'A', (char)'B');
  printf("%f\n", f);
  printf("%d\n", b);
  printf("%ld\n", l);
  printf("%x %x\n", uc, us);
  // A narrow argument between wide ones: the slot a value lands in has to be
  // the one the callee walks to, alignment included.
  printf("%d %f %d\n", c, f, s);
  printf("%d %d %d\n", 1, 2, 3);
  return 0;
}
