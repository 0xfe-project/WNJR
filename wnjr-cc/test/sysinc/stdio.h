// Stub for the dump test only: this machine (macOS) has no /usr/include.
// Declares what vendor tests use; both compilers see the same file.
#ifndef WNJR_STUB_STDIO_H
#define WNJR_STUB_STDIO_H
#ifndef NULL
#define NULL ((void *)0)
#endif
int printf(char *fmt, ...);
int sprintf(char *buf, char *fmt, ...);
#endif
