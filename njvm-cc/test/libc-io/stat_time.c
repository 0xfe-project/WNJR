/* A program that only works if the libc's file and time layers are real.
 *
 * It runs on WNJR through njvm-cli run, with the working directory preopened, so
 * every call below reaches the wasi host: stat -> path_filestat_get, localtime
 * -> the arithmetic in musl's time set, dirname -> pure string work.
 *
 * The harness creates the two paths it looks at, so the sizes and the kinds are
 * known before the run, and every line it prints is compared against them.
 */
#include <libgen.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>

int main(void) {
  int fails = 0;

  /* A regular file whose size the harness fixed when it wrote it. */
  struct stat st;
  if (stat("probe-data.txt", &st) != 0) {
    printf("stat probe-data.txt failed\n");
    return 1;
  }
  printf("data: size=%ld reg=%d dir=%d\n", (long)st.st_size,
         S_ISREG(st.st_mode) ? 1 : 0, S_ISDIR(st.st_mode) ? 1 : 0);

  /* A directory, to see the mode come through as a different kind. */
  struct stat ds;
  if (stat("probe-dir", &ds) != 0) {
    printf("stat probe-dir failed\n");
    return 1;
  }
  printf("dir: dir=%d reg=%d\n", S_ISDIR(ds.st_mode) ? 1 : 0,
         S_ISREG(ds.st_mode) ? 1 : 0);
  if (!S_ISDIR(ds.st_mode) || S_ISREG(ds.st_mode)) fails++;

  /* A path that is not there, to see the failure reported and not a zero
   * structure with a success return. */
  struct stat ns;
  int r = stat("probe-absent", &ns);
  printf("absent: ret=%d\n", r);
  if (r == 0) fails++;

  if (st.st_size != 1234 || !S_ISREG(st.st_mode)) fails++;

  /* 2023-11-14 22:13:20 UTC. wasi-libc's __tz reads no tzfile, so localtime is
   * UTC here, and the answer is fixed rather than whatever today is. */
  time_t t = (time_t)1700000000;
  struct tm tm;
  if (localtime_r(&t, &tm) == NULL) {
    printf("localtime_r failed\n");
    return 1;
  }
  printf("tm: %d-%02d-%02d %02d:%02d:%02d wday=%d yday=%d isdst=%d\n",
         tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min,
         tm.tm_sec, tm.tm_wday, tm.tm_yday, tm.tm_isdst);

  /* ctime_r formats the same instant, and it must agree with the fields above
   * or one of the two paths is lying. */
  char buf[32];
  if (ctime_r(&t, buf) == NULL) {
    printf("ctime_r failed\n");
    return 1;
  }
  printf("ctime: %s", buf);

  /* dirname and basename edit the buffer they are given, so each gets its own
   * copy of the path. */
  char p1[] = "/usr/lib/libc.a";
  char p2[] = "/usr/lib/libc.a";
  char *d = dirname(p1);
  char *b = basename(p2);
  printf("split: dir=%s base=%s\n", d, b);
  if (strcmp(d, "/usr/lib") != 0 || strcmp(b, "libc.a") != 0) fails++;

  printf("fails=%d\n", fails);
  return fails;
}
