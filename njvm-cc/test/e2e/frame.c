// A frame has to come back on every path out of a function, including the one a
// body that runs off its end takes. That path used to return without giving the
// frame back, so each call to such a function leaked it; a loop of them walked
// the stack pointer down through the stack and into the data area, and every
// answer after that came from corrupted memory.

static int counter;
static int calls;

// No return statement, and a local of its own, so it has a frame to leak. C
// leaves the caller's value undefined here; chibicc's own test suite has such a
// function and takes the same risk.
static void bump(int n) {
  int pad[8];
  pad[0] = n;
  pad[7] = pad[0] + 1;
  counter += pad[7] - 1;
}

int frame(int n) {
  counter = 0;
  calls = 0;
  for (int i = 0; i < n; i++) {
    bump(i & 3);
    calls++;
  }
  return counter * 1000 + calls;
}
