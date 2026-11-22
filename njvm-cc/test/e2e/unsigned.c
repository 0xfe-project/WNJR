// A signed and an unsigned value of the same width are different machine
// operations, and the frontend produces the op from the C type: -1 read as an
// unsigned int is the largest value there is, so it is greater than 1, it
// divided by 2 is 2147483647, and shifted right by four it brings in zeros.
int ucmp(int a, int b) { return (unsigned)a > (unsigned)b; }
int scmp(int a, int b) { return a > b; }
int udiv(unsigned a, unsigned b) { return a / b; }
int umod(unsigned a, unsigned b) { return a % b; }
int ushr(unsigned a) { return a >> 4; }
int sshr(int a) { return a >> 4; }
unsigned uadd(unsigned a, unsigned b) { return a + b; }
int ult(unsigned a, unsigned b) { return a < b; }

// The driver declares every call int(int, ...), so the 64-bit cases go through
// int wrappers that widen the argument the way the caller would have.
int uge64(unsigned long a, unsigned long b) { return a >= b; }
long udiv64(unsigned long a, unsigned long b) { return a / b; }
int uge64w(int a, int b) { return uge64((unsigned long)a, (unsigned long)b); }
int udiv64w(int a, int b) { return (int)udiv64((unsigned long)a, (unsigned long)b); }
