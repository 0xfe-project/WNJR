// Labels-as-values: ND_LABEL_VAL (&&label) and ND_GOTO_EXPR (goto *expr).
int direct(void) {
  void *p = &&target;
  int i = 0;
  goto *p;
  i = 100;
target:
  i++;
  return i;
}

int table(int k) {
  static void *tbl[] = {&&a, &&b, &&c};
  int r = 0;
  goto *tbl[k];
a: r += 1;
b: r += 10;
c: r += 100;
  return r;
}

int main(void) {
  return direct() + table(1);
}
