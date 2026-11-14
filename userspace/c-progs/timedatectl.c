// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// timedatectl: the clock and the time zone (step 17), as systemd's.
//
//   timedatectl [status]             local and universal time, the zone
//   timedatectl list-timezones       the zones Rustux knows
//   timedatectl set-timezone ZONE    /etc/timezone
//   timedatectl set-time "YYYY-MM-DD HH:MM:SS" | "HH:MM:SS"   (local time)
//   timedatectl set-local-rtc 0|1    the hardware clock keeps UTC (0) or
//                                    local time, as Windows does (1): /etc/adjtime
//
// Changes are root's (sudo timedatectl ...).

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "sysreq.h"

#define ZONES "/usr/share/zoneinfo/zones"

static int read_line(const char *path, char *buf, size_t cap) {
    FILE *f = fopen(path, "r");
    if (!f) return -1;
    if (!fgets(buf, (int)cap, f)) buf[0] = 0;
    fclose(f);
    buf[strcspn(buf, "\r\n")] = 0;
    return 0;
}

static int write_file(const char *path, const char *text) {
    FILE *f = fopen(path, "w");
    if (!f) {
        fprintf(stderr, "timedatectl: %s: %s%s\n", path, strerror(errno), errno == EACCES ? " (try sudo)" : "");
        return 1;
    }
    fputs(text, f);
    return fclose(f) ? 1 : 0;
}

static int known_zone(const char *z) {
    FILE *f = fopen(ZONES, "r");
    char line[256];
    int found = 0;
    while (f && !found && fgets(line, sizeof line, f)) {
        if (line[0] == '#') continue;
        size_t n = strcspn(line, " \t\n");
        found = n == strlen(z) && !strncmp(line, z, n);
    }
    if (f) fclose(f);
    return found;
}

static int status(void) {
    time_t now = time(0);
    struct tm lt, ut;
    localtime_r(&now, &lt);
    gmtime_r(&now, &ut);
    char a[64], b[64], zone[128] = "UTC", rtc[256] = "";
    strftime(a, sizeof a, "%a %Y-%m-%d %H:%M:%S %Z", &lt);
    strftime(b, sizeof b, "%a %Y-%m-%d %H:%M:%S UTC", &ut);
    read_line("/etc/timezone", zone, sizeof zone);
    if (!zone[0]) strcpy(zone, "UTC");
    FILE *f = fopen("/etc/adjtime", "r");
    if (f) {
        size_t n = fread(rtc, 1, sizeof rtc - 1, f);
        rtc[n] = 0;
        fclose(f);
    }
    char off[16];
    strftime(off, sizeof off, "%z", &lt);
    printf("               Local time: %s\n", a);
    printf("           Universal time: %s\n", b);
    printf("                Time zone: %s (%s, %s)\n", zone, lt.tm_zone ? lt.tm_zone : "UTC", off);
    printf("          RTC in local TZ: %s\n", strstr(rtc, "LOCAL") ? "yes" : "no");
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 1 || !strcmp(argv[1], "status")) return status();
    const char *cmd = argv[1];
    if (!strcmp(cmd, "--help") || !strcmp(cmd, "-h")) {
        printf("usage: timedatectl [status | list-timezones | set-timezone ZONE | set-time TIME | set-local-rtc 0|1]\n");
        return 0;
    }
    if (!strcmp(cmd, "list-timezones")) {
        FILE *f = fopen(ZONES, "r");
        char line[256];
        while (f && fgets(line, sizeof line, f)) {
            if (line[0] == '#' || line[0] == '\n') continue;
            line[strcspn(line, " \t\n")] = 0;
            puts(line);
        }
        if (f) fclose(f);
        return f ? 0 : 1;
    }
    if (argc != 3) {
        fprintf(stderr, "timedatectl: %s needs one argument\n", cmd);
        return 2;
    }
    if (!strcmp(cmd, "set-timezone")) {
        if (!known_zone(argv[2])) {
            fprintf(stderr, "timedatectl: unknown time zone %s (see timedatectl list-timezones)\n", argv[2]);
            return 1;
        }
        char text[160];
        snprintf(text, sizeof text, "%s\n", argv[2]);
        return write_file("/etc/timezone", text);
    }
    if (!strcmp(cmd, "set-local-rtc")) {
        int local = !strcmp(argv[2], "1") || !strcmp(argv[2], "yes") || !strcmp(argv[2], "true");
        if (!local && strcmp(argv[2], "0") && strcmp(argv[2], "no") && strcmp(argv[2], "false")) {
            fprintf(stderr, "timedatectl: set-local-rtc takes 0 or 1\n");
            return 2;
        }
        return write_file("/etc/adjtime", local ? "0.0 0 0\n0\nLOCAL\n" : "0.0 0 0\n0\nUTC\n");
    }
    if (!strcmp(cmd, "set-time")) {
        time_t now = time(0);
        struct tm tm;
        localtime_r(&now, &tm);
        // Numbers separated by '-', ' ' and ':': a date first if there is a '-'
        long v[6] = { 0, 0, 0, 0, 0, 0 };
        int nv = 0;
        const char *p = argv[2];
        while (*p && nv < 6) {
            char *end;
            v[nv++] = strtol(p, &end, 10);
            if (end == p) { nv = -1; break; }
            p = end;
            while (*p == '-' || *p == ':' || *p == ' ' || *p == 'T') p++;
        }
        int dated = strchr(argv[2], '-') != 0;
        if (nv < 0 || *p || (dated ? nv < 5 : nv < 2)) {
            fprintf(stderr, "timedatectl: a time is \"YYYY-MM-DD HH:MM:SS\" or \"HH:MM:SS\"\n");
            return 2;
        }
        long *t0 = v;
        if (dated) {
            tm.tm_year = (int)v[0] - 1900;
            tm.tm_mon = (int)v[1] - 1;
            tm.tm_mday = (int)v[2];
            t0 = v + 3;
        }
        int h = (int)t0[0], mi = (int)t0[1], s = (int)t0[2];
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = s;
        tm.tm_isdst = -1;
        time_t t = mktime(&tm);
        if (t == (time_t)-1) {
            fprintf(stderr, "timedatectl: not a valid time\n");
            return 1;
        }
        int64_t ns = (int64_t)t * 1000000000;
        int r = sysreq(RX_SYSTEM_OP_SET_TIME, &ns, sizeof ns);
        if (r < 0) {
            fprintf(stderr, "timedatectl: setting the clock: %s%s\n", strerror(-r), r == -EPERM ? " (try sudo)" : "");
            return 1;
        }
        return 0;
    }
    fprintf(stderr, "timedatectl: unknown command %s\n", cmd);
    return 2;
}
