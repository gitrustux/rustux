// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// logger: put a message in the system log (step 18)
//
//   logger [-t TAG] [-p [FACILITY.]PRIO] [-s] [-f FILE] [MESSAGE...]
//
// Without MESSAGE (or with -f), each line of stdin (or FILE) is one.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <syslog.h>
#include <unistd.h>

static int prio_of(const char *s) {
    static const char *const names[8] = { "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug" };
    const char *dot = strchr(s, '.');
    if (dot) s = dot + 1; // the facility is accepted and ignored
    if (s[0] >= '0' && s[0] <= '7' && !s[1]) return s[0] - '0';
    for (int i = 0; i < 8; i++) if (!strcmp(s, names[i])) return i;
    if (!strcmp(s, "error")) return 3;
    if (!strcmp(s, "warn")) return 4;
    if (!strcmp(s, "panic")) return 0;
    return -1;
}

int main(int argc, char **argv) {
    const char *tag = 0, *file = 0;
    int prio = LOG_NOTICE, opt = 0, i = 1;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "--")) { i++; break; }
        if ((!strcmp(a, "-t") || !strcmp(a, "--tag")) && i + 1 < argc) tag = argv[++i];
        else if ((!strcmp(a, "-p") || !strcmp(a, "--priority")) && i + 1 < argc) {
            if ((prio = prio_of(argv[++i])) < 0) {
                fprintf(stderr, "logger: unknown priority %s\n", argv[i]);
                return 1;
            }
        } else if (!strcmp(a, "-s") || !strcmp(a, "--stderr")) opt |= LOG_PERROR;
        else if ((!strcmp(a, "-f") || !strcmp(a, "--file")) && i + 1 < argc) file = argv[++i];
        else if (!strcmp(a, "-i") || !strcmp(a, "--id")) { /* the pid is always recorded */ }
        else if (!strcmp(a, "-h") || !strcmp(a, "--help")) {
            printf("usage: logger [-t TAG] [-p [FACILITY.]PRIO] [-s] [-f FILE] [MESSAGE...]\n"
                   "Put a message (or each line of stdin or FILE) in the system log.\n");
            return 0;
        } else {
            fprintf(stderr, "usage: logger [-t TAG] [-p [FACILITY.]PRIO] [-s] [-f FILE] [MESSAGE...]\n");
            return 1;
        }
    }
    if (!tag) tag = getlogin() ? getlogin() : "logger";
    openlog(tag, opt | LOG_NDELAY, LOG_USER);
    if (i < argc && !file) {
        char msg[2000] = "";
        size_t n = 0;
        for (; i < argc && n < sizeof msg - 1; i++) n += (size_t)snprintf(msg + n, sizeof msg - n, "%s%s", n ? " " : "", argv[i]);
        syslog(prio, "%s", msg);
        return 0;
    }
    FILE *f = file ? fopen(file, "r") : stdin;
    if (!f) {
        perror(file);
        return 1;
    }
    char line[2000];
    while (fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        if (line[0]) syslog(prio, "%s", line);
    }
    return 0;
}
