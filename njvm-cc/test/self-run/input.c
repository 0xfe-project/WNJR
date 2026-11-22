// What the guest compiler is asked to compile. Nothing here needs a header or a
// library, so the output depends only on the front end: a struct with a member
// offset, a switch that becomes a jump table, a loop, and a pointer argument.
struct point {
  int x;
  int y;
};

static int area(struct point *p) { return p->x * p->y; }

int classify(int n) {
  switch (n) {
  case 0:
    return 1;
  case 1:
    return 3;
  case 2:
    return 9;
  default:
    return n * 2;
  }
}

int main(void) {
  struct point p = {3, 4};
  int s = 0;
  for (int i = 0; i < 10; i++) s += classify(i) + area(&p);
  return s;
}
