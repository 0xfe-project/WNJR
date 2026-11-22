/* musl's asctime_r reaches the day and month names through __nl_langinfo_l,
 * and the langinfo in this tree has the C locale only, whose names are these
 * fixed strings. So they are written out here: same output, no locale layer,
 * and no __nl_langinfo_l, which this tree does not compile. */
#include <time.h>
#include <stdio.h>

static const char wday_name[7][4] = {
    "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
static const char mon_name[12][4] = {
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};

char *__asctime_r(const struct tm *restrict tm, char *restrict buf) {
  snprintf(buf, 26, "%.3s %.3s%3d %.2d:%.2d:%.2d %d\n",
           wday_name[tm->tm_wday], mon_name[tm->tm_mon], tm->tm_mday,
           tm->tm_hour, tm->tm_min, tm->tm_sec, 1900 + tm->tm_year);
  return buf;
}

char *asctime_r(const struct tm *restrict tm, char *restrict buf) {
  return __asctime_r(tm, buf);
}

char *asctime(const struct tm *tm) {
  static char buf[26];
  return __asctime_r(tm, buf);
}
