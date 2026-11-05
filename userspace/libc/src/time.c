// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Time: clocks, calendar conversion, time zones, strftime, uname.
//
// Time zones are POSIX TZ rules ("CST6CDT,M3.2.0,M11.1.0"). The zone comes
// from $TZ, else from /etc/timezone; a zone name such as America/Chicago
// is looked up in /usr/share/zoneinfo/zones, a table of names and rules
// (no tzdata history: today's rules apply to every year).

#include "libc.h"
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

time_t time(time_t *t) {
    time_t now = (time_t)(rx_clock_get_utc() / 1000000000);
    if (t) *t = now;
    return now;
}

int clock_gettime(clockid_t clock, struct timespec *ts) {
    int64_t ns;
    if (clock == CLOCK_REALTIME) ns = rx_clock_get_utc();
    else if (clock == CLOCK_MONOTONIC) ns = (int64_t)rx_clock_get_monotonic();
    else { errno = EINVAL; return -1; }
    ts->tv_sec = ns / 1000000000;
    ts->tv_nsec = ns % 1000000000;
    return 0;
}

int gettimeofday(struct timeval *restrict tv, void *restrict tz) {
    (void)tz;
    int64_t ns = rx_clock_get_utc();
    tv->tv_sec = ns / 1000000000;
    tv->tv_usec = (ns % 1000000000) / 1000;
    return 0;
}

int nanosleep(const struct timespec *req, struct timespec *rem) {
    if (req->tv_sec < 0 || req->tv_nsec < 0 || req->tv_nsec >= 1000000000) { errno = EINVAL; return -1; }
    rx_handle_t t;
    if (rx_timer_create(0, 0, &t) != RX_OK) { errno = ENOMEM; return -1; }
    rx_timer_set(t, rx_clock_get_monotonic() + (uint64_t)req->tv_sec * 1000000000ull + (uint64_t)req->tv_nsec, 0);
    rx_object_wait_one(t, RX_TIMER_SIGNALED, RX_TIME_INFINITE, 0);
    rx_handle_close(t);
    if (rem) rem->tv_sec = rem->tv_nsec = 0;
    return 0;
}

clock_t clock(void) {
    rx_info_task_runtime_t rt;
    if (rx_object_get_info(rx_proc_self, RX_INFO_TASK_RUNTIME, &rt, sizeof rt, 0, 0) != RX_OK) return -1;
    return (clock_t)(rt.cpu_time / 1000);
}

double difftime(time_t a, time_t b) { return (double)(a - b); }

// ---- calendar ----

static int leap(long y) { return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }
static const int mdays[12] = { 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 };
static int days_in_month(long y, int m) { return mdays[m] + (m == 1 && leap(y)); }

// Days since 1970-01-01 for y-m-d (m 1-12)
static long days_from_civil(long y, long m, long d) {
    y -= m <= 2;
    long era = (y >= 0 ? y : y - 399) / 400;
    long yoe = y - era * 400;
    long doy = (153 * (m + (m > 2 ? -3 : 9)) + 2) / 5 + d - 1;
    long doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    return era * 146097 + doe - 719468;
}

static void civil_from_days(long z, long *y, int *m, int *d) {
    z += 719468;
    long era = (z >= 0 ? z : z - 146096) / 146097;
    long doe = z - era * 146097;
    long yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    long doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    long mp = (5 * doy + 2) / 153;
    *d = (int)(doy - (153 * mp + 2) / 5 + 1);
    *m = (int)(mp < 10 ? mp + 3 : mp - 9);
    *y = yoe + era * 400 + (*m <= 2);
}

static void fill_tm(time_t t, long offset, int isdst, const char *zone, struct tm *out) {
    t += offset;
    long days = t / 86400, secs = t % 86400;
    if (secs < 0) { secs += 86400; days--; }
    long y;
    int m, d;
    civil_from_days(days, &y, &m, &d);
    out->tm_year = (int)(y - 1900);
    out->tm_mon = m - 1;
    out->tm_mday = d;
    out->tm_hour = (int)(secs / 3600);
    out->tm_min = (int)(secs / 60 % 60);
    out->tm_sec = (int)(secs % 60);
    out->tm_wday = (int)(((days % 7) + 11) % 7); // 1970-01-01 was a Thursday
    out->tm_yday = (int)(days - days_from_civil(y, 1, 1));
    out->tm_isdst = isdst;
    out->tm_gmtoff = offset;
    out->tm_zone = zone;
}

time_t timegm(struct tm *tm) {
    // Normalise month overflow, then let the day count absorb the rest
    long y = tm->tm_year + 1900L + tm->tm_mon / 12;
    int m = tm->tm_mon % 12;
    if (m < 0) { m += 12; y--; }
    time_t t = (time_t)days_from_civil(y, m + 1, 1) * 86400 + (time_t)(tm->tm_mday - 1) * 86400 +
               (time_t)tm->tm_hour * 3600 + (time_t)tm->tm_min * 60 + tm->tm_sec;
    fill_tm(t, 0, 0, "UTC", tm);
    return t;
}

struct tm *gmtime_r(const time_t *restrict t, struct tm *restrict out) {
    fill_tm(*t, 0, 0, "UTC", out);
    return out;
}

// ---- time zones ----

struct rule { int m, w, d; long time; }; // Mm.w.d/time
static struct {
    char key[128];            // the TZ value this was parsed from
    char std[16], dst[16];
    long std_off, dst_off;    // seconds east of UTC
    int has_dst;
    struct rule start, end;
} tz = { "", "UTC", "UTC", 0, 0, 0, { 0, 0, 0, 0 }, { 0, 0, 0, 0 } };

char *tzname[2] = { tz.std, tz.dst };
long timezone;
int daylight;

static const char *parse_name(const char *p, char *out) {
    size_t n = 0;
    if (*p == '<') {
        p++;
        while (*p && *p != '>' && n < 15) out[n++] = *p++;
        if (*p == '>') p++;
    } else {
        while (isalpha((unsigned char)*p) && n < 15) out[n++] = *p++;
    }
    out[n] = 0;
    return n >= 3 ? p : 0;
}

// [+-]hh[:mm[:ss]]; returns seconds
static const char *parse_hms(const char *p, long *out) {
    int sign = 1;
    if (*p == '+' || *p == '-') sign = *p++ == '-' ? -1 : 1;
    if (!isdigit((unsigned char)*p)) return 0;
    long h = strtol(p, (char **)&p, 10), m = 0, s = 0;
    if (*p == ':') { p++; m = strtol(p, (char **)&p, 10); }
    if (*p == ':') { p++; s = strtol(p, (char **)&p, 10); }
    *out = sign * (h * 3600 + m * 60 + s);
    return p;
}

static const char *parse_rule(const char *p, struct rule *r) {
    if (*p != 'M') return 0; // only the Mm.w.d form
    p++;
    r->m = (int)strtol(p, (char **)&p, 10);
    if (*p++ != '.') return 0;
    r->w = (int)strtol(p, (char **)&p, 10);
    if (*p++ != '.') return 0;
    r->d = (int)strtol(p, (char **)&p, 10);
    r->time = 7200;
    if (*p == '/') p = parse_hms(p + 1, &r->time);
    if (!p || r->m < 1 || r->m > 12 || r->w < 1 || r->w > 5 || r->d < 0 || r->d > 6) return 0;
    return p;
}

// Parse a POSIX TZ string into tz; 0 on success
static int parse_posix(const char *s) {
    char std[16], dst[16];
    long off, doff;
    struct rule a, b;
    const char *p = parse_name(s, std);
    if (!p || !(p = parse_hms(p, &off))) return -1;
    off = -off; // POSIX offsets are west of UTC
    int has_dst = 0;
    if (*p) {
        p = parse_name(p, dst);
        if (!p) return -1;
        doff = off + 3600;
        if (*p && *p != ',') {
            if (!(p = parse_hms(p, &doff))) return -1;
            doff = -doff;
        }
        // Default rules (US) if none are given
        a = (struct rule){ 3, 2, 0, 7200 };
        b = (struct rule){ 11, 1, 0, 7200 };
        if (*p == ',') {
            if (!(p = parse_rule(p + 1, &a)) || *p++ != ',' || !(p = parse_rule(p, &b))) return -1;
        }
        has_dst = 1;
    }
    strcpy(tz.std, std);
    strcpy(tz.dst, has_dst ? dst : std);
    tz.std_off = off;
    tz.dst_off = has_dst ? doff : off;
    tz.has_dst = has_dst;
    if (has_dst) { tz.start = a; tz.end = b; }
    return 0;
}

static int read_line(const char *path, char *buf, size_t cap) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    ssize_t n = read(fd, buf, cap - 1);
    close(fd);
    if (n <= 0) return -1;
    buf[n] = 0;
    buf[strcspn(buf, "\r\n")] = 0;
    return 0;
}

// The rule for a zone name from the zones table
static int lookup_zone(const char *name, char *rule, size_t cap) {
    int fd = open("/usr/share/zoneinfo/zones", O_RDONLY);
    if (fd < 0) return -1;
    static char table[16384];
    ssize_t n = read(fd, table, sizeof table - 1);
    close(fd);
    if (n <= 0) return -1;
    table[n] = 0;
    size_t len = strlen(name);
    for (char *line = table; *line;) {
        char *next = strchr(line, '\n');
        if (next) *next = 0;
        if (!strncmp(line, name, len) && (line[len] == ' ' || line[len] == '\t')) {
            char *r = line + len;
            while (*r == ' ' || *r == '\t') r++;
            snprintf(rule, cap, "%s", r);
            return 0;
        }
        if (!next) break;
        line = next + 1;
    }
    return -1;
}

void tzset(void) {
    char val[128] = "";
    const char *env = getenv("TZ");
    if (env && *env) snprintf(val, sizeof val, "%s", env + (*env == ':'));
    else if (read_line("/etc/timezone", val, sizeof val) < 0) strcpy(val, "UTC");
    if (!strcmp(val, tz.key)) return;
    snprintf(tz.key, sizeof tz.key, "%s", val);
    char rule[128];
    if (parse_posix(val) < 0) {
        if (lookup_zone(val, rule, sizeof rule) < 0 || parse_posix(rule) < 0) parse_posix("UTC0");
    }
    timezone = -tz.std_off;
    daylight = tz.has_dst;
}

// Day of month of rule `r` in year y
static int rule_mday(long y, const struct rule *r) {
    int first = (int)(((days_from_civil(y, r->m, 1) % 7) + 11) % 7); // weekday of the 1st
    int d = 1 + (r->d - first + 7) % 7 + (r->w - 1) * 7;
    while (d > days_in_month(y, r->m - 1)) d -= 7;
    return d;
}

// Is UTC time t in daylight saving time?
static int in_dst(time_t t) {
    if (!tz.has_dst) return 0;
    struct tm lt;
    fill_tm(t, tz.std_off, 0, 0, &lt);
    long y = lt.tm_year + 1900L;
    // Transitions as UTC: the start is given in standard time, the end in DST
    time_t start = (time_t)days_from_civil(y, tz.start.m, rule_mday(y, &tz.start)) * 86400 + tz.start.time - tz.std_off;
    time_t end = (time_t)days_from_civil(y, tz.end.m, rule_mday(y, &tz.end)) * 86400 + tz.end.time - tz.dst_off;
    return start < end ? (t >= start && t < end) : (t >= start || t < end);
}

struct tm *localtime_r(const time_t *restrict t, struct tm *restrict out) {
    tzset();
    int dst = in_dst(*t);
    fill_tm(*t, dst ? tz.dst_off : tz.std_off, dst, dst ? tz.dst : tz.std, out);
    return out;
}

time_t mktime(struct tm *tm) {
    tzset();
    struct tm copy = *tm;
    time_t local = timegm(&copy); // the fields as if they were UTC
    time_t t = local - tz.std_off;
    if (tm->tm_isdst > 0 || (tm->tm_isdst < 0 && in_dst(local - tz.dst_off))) t = local - tz.dst_off;
    localtime_r(&t, tm);
    return t;
}

struct tm *gmtime(const time_t *t) {
    static struct tm r;
    return gmtime_r(t, &r);
}

struct tm *localtime(const time_t *t) {
    static struct tm r;
    return localtime_r(t, &r);
}

// ---- formatting ----

static const char *const wday_names[7] = { "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday" };
static const char *const mon_names[12] = { "January", "February", "March", "April", "May", "June", "July",
                                           "August", "September", "October", "November", "December" };

size_t strftime(char *restrict s, size_t max, const char *restrict fmt, const struct tm *restrict tm) {
    size_t n = 0;
    char tmp[64];
#define PUT(str) do { const char *q_ = (str); size_t k_ = strlen(q_); if (n + k_ >= max) return 0; memcpy(s + n, q_, k_); n += k_; } while (0)
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            if (n + 1 >= max) return 0;
            s[n++] = *fmt;
            continue;
        }
        fmt++;
        if (*fmt == 'E' || *fmt == 'O') fmt++; // modifiers: ignored
        int h12 = tm->tm_hour % 12 ? tm->tm_hour % 12 : 12;
        switch (*fmt) {
        case 'a': snprintf(tmp, sizeof tmp, "%.3s", wday_names[tm->tm_wday % 7]); PUT(tmp); break;
        case 'A': PUT(wday_names[tm->tm_wday % 7]); break;
        case 'b': case 'h': snprintf(tmp, sizeof tmp, "%.3s", mon_names[tm->tm_mon % 12]); PUT(tmp); break;
        case 'B': PUT(mon_names[tm->tm_mon % 12]); break;
        case 'c': strftime(tmp, sizeof tmp, "%a %b %e %H:%M:%S %Y", tm); PUT(tmp); break;
        case 'C': snprintf(tmp, sizeof tmp, "%02d", (tm->tm_year + 1900) / 100); PUT(tmp); break;
        case 'd': snprintf(tmp, sizeof tmp, "%02d", tm->tm_mday); PUT(tmp); break;
        case 'D': strftime(tmp, sizeof tmp, "%m/%d/%y", tm); PUT(tmp); break;
        case 'e': snprintf(tmp, sizeof tmp, "%2d", tm->tm_mday); PUT(tmp); break;
        case 'F': strftime(tmp, sizeof tmp, "%Y-%m-%d", tm); PUT(tmp); break;
        case 'H': snprintf(tmp, sizeof tmp, "%02d", tm->tm_hour); PUT(tmp); break;
        case 'I': snprintf(tmp, sizeof tmp, "%02d", h12); PUT(tmp); break;
        case 'j': snprintf(tmp, sizeof tmp, "%03d", tm->tm_yday + 1); PUT(tmp); break;
        case 'k': snprintf(tmp, sizeof tmp, "%2d", tm->tm_hour); PUT(tmp); break;
        case 'l': snprintf(tmp, sizeof tmp, "%2d", h12); PUT(tmp); break;
        case 'm': snprintf(tmp, sizeof tmp, "%02d", tm->tm_mon + 1); PUT(tmp); break;
        case 'M': snprintf(tmp, sizeof tmp, "%02d", tm->tm_min); PUT(tmp); break;
        case 'n': PUT("\n"); break;
        case 'p': PUT(tm->tm_hour < 12 ? "AM" : "PM"); break;
        case 'P': PUT(tm->tm_hour < 12 ? "am" : "pm"); break;
        case 'r': strftime(tmp, sizeof tmp, "%I:%M:%S %p", tm); PUT(tmp); break;
        case 'R': strftime(tmp, sizeof tmp, "%H:%M", tm); PUT(tmp); break;
        case 's': {
            struct tm c = *tm;
            snprintf(tmp, sizeof tmp, "%lld", (long long)(timegm(&c) - tm->tm_gmtoff));
            PUT(tmp);
            break;
        }
        case 'S': snprintf(tmp, sizeof tmp, "%02d", tm->tm_sec); PUT(tmp); break;
        case 't': PUT("\t"); break;
        case 'T': strftime(tmp, sizeof tmp, "%H:%M:%S", tm); PUT(tmp); break;
        case 'u': snprintf(tmp, sizeof tmp, "%d", tm->tm_wday ? tm->tm_wday : 7); PUT(tmp); break;
        case 'w': snprintf(tmp, sizeof tmp, "%d", tm->tm_wday); PUT(tmp); break;
        case 'x': strftime(tmp, sizeof tmp, "%m/%d/%y", tm); PUT(tmp); break;
        case 'X': strftime(tmp, sizeof tmp, "%H:%M:%S", tm); PUT(tmp); break;
        case 'y': snprintf(tmp, sizeof tmp, "%02d", (tm->tm_year + 1900) % 100); PUT(tmp); break;
        case 'Y': snprintf(tmp, sizeof tmp, "%d", tm->tm_year + 1900); PUT(tmp); break;
        case 'z': {
            long o = tm->tm_gmtoff;
            snprintf(tmp, sizeof tmp, "%c%02ld%02ld", o < 0 ? '-' : '+', labs(o) / 3600, labs(o) / 60 % 60);
            PUT(tmp);
            break;
        }
        case 'Z': PUT(tm->tm_zone ? tm->tm_zone : ""); break;
        case '%': PUT("%"); break;
        case 0: fmt--; break;
        default: tmp[0] = '%'; tmp[1] = *fmt; tmp[2] = 0; PUT(tmp); break;
        }
    }
#undef PUT
    s[n] = 0;
    return n;
}

char *asctime_r(const struct tm *restrict tm, char *restrict buf) {
    strftime(buf, 26, "%a %b %e %H:%M:%S %Y\n", tm);
    return buf;
}

char *ctime_r(const time_t *t, char *buf) {
    struct tm tm;
    return asctime_r(localtime_r(t, &tm), buf);
}

char *asctime(const struct tm *tm) {
    static char buf[32];
    return asctime_r(tm, buf);
}

char *ctime(const time_t *t) {
    static char buf[32];
    return ctime_r(t, buf);
}

// ---- system names ----

int gethostname(char *name, size_t len) {
    char buf[65];
    if (read_line("/etc/hostname", buf, sizeof buf) < 0 || !buf[0]) strcpy(buf, "rustica");
    if (strlen(buf) >= len) { errno = ENAMETOOLONG; return -1; }
    strcpy(name, buf);
    return 0;
}

int uname(struct utsname *u) {
    char version[128] = "";
    memset(u, 0, sizeof *u);
    rx_system_get_version(version, sizeof version);
    strcpy(u->sysname, "Rustux");
    if (gethostname(u->nodename, sizeof u->nodename) < 0) strcpy(u->nodename, "rustica");
    // "Rustux 0.1.0 (git abc1234, built 2026-11-04) x86_64"
    const char *rel = strchr(version, ' ');
    snprintf(u->release, sizeof u->release, "%.*s", rel ? (int)strcspn(rel + 1, " ") : 0, rel ? rel + 1 : "");
    const char *paren = strchr(version, '(');
    snprintf(u->version, sizeof u->version, "%.*s", paren ? (int)strcspn(paren, ")") + 1 : 0, paren ? paren : "");
    strcpy(u->machine, "x86_64");
    return 0;
}

pid_t getpid(void) {
    rx_info_handle_basic_t b;
    if (rx_object_get_info(rx_proc_self, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0) != RX_OK) return -1;
    return (pid_t)b.koid;
}
