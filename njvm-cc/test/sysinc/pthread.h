// Stub for the dump test only: this machine (macOS) has no /usr/include.
#ifndef WNJR_STUB_PTHREAD_H
#define WNJR_STUB_PTHREAD_H
#define NULL ((void *)0)
typedef unsigned long pthread_t;
int pthread_create(pthread_t *thr, void *attr, int (*fn)(void *), void *arg);
int pthread_join(pthread_t thr, void *ret);
#endif
