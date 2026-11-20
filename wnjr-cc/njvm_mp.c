// MessagePack writer for the NJVM-IR interchange format. See njvm.h.

#include "chibicc.h"
#include "njvm.h"

#include <stdlib.h>
#include <string.h>

static noreturn void oom(void) {
  error("out of memory");
}

void mp_init(Mp *m) {
  m->cap = 1 << 16;
  m->len = 0;
  m->data = malloc(m->cap);
  if (!m->data)
    oom();
}

void mp_free(Mp *m) {
  free(m->data);
  m->data = NULL;
  m->len = m->cap = 0;
}

static void reserve(Mp *m, size_t n) {
  if (m->len + n <= m->cap)
    return;
  size_t cap = m->cap;
  while (cap < m->len + n)
    cap *= 2;
  unsigned char *p = realloc(m->data, cap);
  if (!p)
    oom();
  m->data = p;
  m->cap = cap;
}

static void put(Mp *m, unsigned char c) {
  reserve(m, 1);
  m->data[m->len++] = c;
}

static void put_bytes(Mp *m, const void *p, size_t n) {
  reserve(m, n);
  memcpy(m->data + m->len, p, n);
  m->len += n;
}

static void put_be32(Mp *m, uint32_t v) {
  unsigned char b[4] = {v >> 24, v >> 16, v >> 8, v};
  put_bytes(m, b, 4);
}

static void put_be64(Mp *m, uint64_t v) {
  unsigned char b[8];
  for (int i = 0; i < 8; i++)
    b[i] = (unsigned char)(v >> (56 - 8 * i));
  put_bytes(m, b, 8);
}

// A 32-bit container header with a zero count, patched by mp_end_container.
static int begin_container(Mp *m, unsigned char tag) {
  put(m, tag);
  int marker = (int)m->len;
  put_be32(m, 0);
  return marker;
}

int mp_begin_map(Mp *m) {
  return begin_container(m, 0xdf);
}

int mp_begin_arr(Mp *m) {
  return begin_container(m, 0xdd);
}

void mp_end_container(Mp *m, int marker, size_t n) {
  if (n > 0xffffffffu)
    oom();
  uint32_t v = (uint32_t)n;
  unsigned char b[4] = {v >> 24, v >> 16, v >> 8, v};
  memcpy(m->data + marker, b, 4);
}

void mp_str(Mp *m, const char *s, size_t len) {
  if (len < 32) {
    put(m, (unsigned char)(0xa0 | len));
  } else if (len < 256) {
    put(m, 0xd9);
    put(m, (unsigned char)len);
  } else if (len < 65536) {
    put(m, 0xda);
    put(m, (unsigned char)(len >> 8));
    put(m, (unsigned char)len);
  } else {
    put(m, 0xdb);
    put_be32(m, (uint32_t)len);
  }
  put_bytes(m, s, len);
}

void mp_key(Mp *m, const char *k) {
  mp_str(m, k, strlen(k));
}

void mp_uint(Mp *m, uint64_t v) {
  if (v < 128) {
    put(m, (unsigned char)v);
  } else if (v < 256) {
    put(m, 0xcc);
    put(m, (unsigned char)v);
  } else if (v < 65536) {
    put(m, 0xcd);
    put(m, (unsigned char)(v >> 8));
    put(m, (unsigned char)v);
  } else if (v < 0x100000000ull) {
    put(m, 0xce);
    put_be32(m, (uint32_t)v);
  } else {
    put(m, 0xcf);
    put_be64(m, v);
  }
}

void mp_int(Mp *m, int64_t v) {
  if (v >= 0) {
    mp_uint(m, (uint64_t)v);
  } else if (v >= -32) {
    put(m, (unsigned char)(0xe0 | (v + 32)));
  } else if (v >= -128) {
    put(m, 0xd0);
    put(m, (unsigned char)(int8_t)v);
  } else if (v >= -32768) {
    put(m, 0xd1);
    put(m, (unsigned char)((uint16_t)v >> 8));
    put(m, (unsigned char)v);
  } else if (v >= -2147483648ll) {
    put(m, 0xd2);
    put_be32(m, (uint32_t)(int32_t)v);
  } else {
    put(m, 0xd3);
    put_be64(m, (uint64_t)v);
  }
}

void mp_nil(Mp *m) {
  put(m, 0xc0);
}

void mp_bool(Mp *m, bool v) {
  put(m, v ? 0xc3 : 0xc2);
}

int mp_variant(Mp *m, const char *name) {
  int marker = mp_begin_map(m);
  mp_key(m, name);
  return marker;
}

void mp_variant_unit(Mp *m, const char *name) {
  mp_str(m, name, strlen(name));
}

void mp_write_file(Mp *m, FILE *out) {
  if (fwrite(m->data, 1, m->len, out) != m->len)
    error("cannot write output: %s", strerror(errno));
}
