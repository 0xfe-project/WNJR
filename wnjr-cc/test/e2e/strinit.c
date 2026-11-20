// A string in braces is still a string initializer. The standard spells
// { "abc" } as an equivalent form of "abc" for an array of char, and the two
// must fill the same bytes; chibicc reads the braces element by element, so the
// whole string landed in one char and the rest stayed zero. musl keeps its
// digit table as { "0123456789ABCDEF" }, and its hexadecimal output printed
// spaces until this worked.

static const char digits[17] = { "0123456789ABCDEF" };
static char local_copy(void) {
  char t[5] = { "abc" };
  return t[3];
}

// Every value below is checked against the host compiler.
int braced_static(void) {
  int r = 0;
  for (int i = 0; i < 16; i++)
    r = r * 3 + digits[i];
  r = r * 3 + digits[16];
  return r;
}

int braced_local(void) {
  char t[6] = { "wxyz" };
  return t[0] * 1000000 + t[1] * 10000 + t[2] * 100 + t[3] + t[4] * 7 + local_copy();
}

int braced_flexible(void) {
  // The length comes from the string, not from a written bound.
  char t[] = { "hello" };
  return sizeof(t) * 100 + t[0] + t[4];
}

int braced_wide(void) {
  // A wide character array takes the same form, one element per code unit.
  int w[4] = { L"ab" };
  return w[0] * 1000000 + w[1] * 10000 + w[2] * 100 + w[3];
}
