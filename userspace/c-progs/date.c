// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// date [-u] [+FORMAT]: print the date and time (local time from $TZ or
// /etc/timezone; -u for UTC). FORMAT as for strftime, e.g. +%Y-%m-%d.

#include <stdio.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    int utc = 0;
    const char *fmt = "%a %b %e %H:%M:%S %Z %Y";
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-u") || !strcmp(argv[i], "--utc")) utc = 1;
        else if (argv[i][0] == '+') fmt = argv[i] + 1;
        else { fprintf(stderr, "usage: date [-u] [+FORMAT]\n"); return 2; }
    }
    time_t now = time(0);
    struct tm tm;
    if (utc) gmtime_r(&now, &tm); else localtime_r(&now, &tm);
    char buf[256];
    strftime(buf, sizeof buf, fmt, &tm);
    puts(buf);
    return 0;
}
