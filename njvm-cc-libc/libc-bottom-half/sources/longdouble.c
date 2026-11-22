// long double is double in this frontend: chibicc gives it eight bytes and the
// NJVM-IR type F64, which is what the target has. The C library still promises
// the l-suffixed spellings, and portable code asks for them by name — musl's
// floatscan is one such caller — so they forward to the double-width function
// instead of being absent. Each forward is exact: there is no precision here to
// lose that this type ever had.
#include <math.h>

long double fabsl(long double x) { return fabs(x); }
long double copysignl(long double x, long double y) { return copysign(x, y); }
long double fmodl(long double x, long double y) { return fmod(x, y); }
long double scalbnl(long double x, int n) { return scalbn(x, n); }
