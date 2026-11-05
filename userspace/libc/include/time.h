// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _TIME_H
#define _TIME_H

#include <stddef.h>
#include <sys/types.h>

#ifndef _STRUCT_TIMESPEC
#define _STRUCT_TIMESPEC
struct timespec { time_t tv_sec; long tv_nsec; };
#endif

struct tm {
    int tm_sec, tm_min, tm_hour;
    int tm_mday, tm_mon, tm_year;   // month 0-11, year - 1900
    int tm_wday, tm_yday, tm_isdst;
    long tm_gmtoff;                 // seconds east of UTC
    const char *tm_zone;
};

typedef int clockid_t;
#define CLOCK_REALTIME  0
#define CLOCK_MONOTONIC 1
#define CLOCKS_PER_SEC 1000000L
typedef long clock_t;

time_t time(time_t *t);
int clock_gettime(clockid_t clock, struct timespec *ts);
int nanosleep(const struct timespec *req, struct timespec *rem);
clock_t clock(void);
double difftime(time_t a, time_t b);

struct tm *gmtime_r(const time_t *restrict t, struct tm *restrict out);
struct tm *localtime_r(const time_t *restrict t, struct tm *restrict out);
struct tm *gmtime(const time_t *t);
struct tm *localtime(const time_t *t);
time_t mktime(struct tm *tm);
time_t timegm(struct tm *tm);
size_t strftime(char *restrict s, size_t max, const char *restrict fmt, const struct tm *restrict tm);
char *asctime_r(const struct tm *restrict tm, char *restrict buf);
char *ctime_r(const time_t *t, char *buf);
char *asctime(const struct tm *tm);
char *ctime(const time_t *t);
void tzset(void);

extern char *tzname[2];
extern long timezone;
extern int daylight;

#endif
