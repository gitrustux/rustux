// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// logfile.h: reading the system log that logd writes (step 18; the format
// is in <rustux/log.h>). logs and svc status use it.

#ifndef LOGFILE_H
#define LOGFILE_H

#include <errno.h>
#include <rustux/log.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct {
    const char *time;   // as stored (UTC)
    long long ms;       // milliseconds since 1970
    unsigned boot, prio;
    const char *tag;
    unsigned long long pid;
    unsigned uid;
    const char *msg;
} logrec_t;

// Split a stored line (changed in place); 0 if it is not one
static __attribute__((unused)) int lf_parse(char *line, logrec_t *r) {
    char *f[7];
    int n = 0;
    f[n++] = line;
    for (char *p = line; *p && n < 7; p++)
        if (*p == '\t') {
            *p = 0;
            f[n++] = p + 1;
        }
    if (n < 7) return 0;
    char *nl = strchr(f[6], '\n');
    if (nl) *nl = 0;
    r->time = f[0];
    struct tm tm;
    memset(&tm, 0, sizeof tm);
    int ms = 0;
    if (strlen(f[0]) < 20) return 0;
    tm.tm_year = atoi(f[0]) - 1900;
    tm.tm_mon = atoi(f[0] + 5) - 1;
    tm.tm_mday = atoi(f[0] + 8);
    tm.tm_hour = atoi(f[0] + 11);
    tm.tm_min = atoi(f[0] + 14);
    tm.tm_sec = atoi(f[0] + 17);
    if (f[0][19] == '.') ms = atoi(f[0] + 20);
    r->ms = (long long)timegm(&tm) * 1000 + ms;
    r->boot = (unsigned)strtoul(f[1], 0, 10);
    r->prio = (unsigned)strtoul(f[2], 0, 10);
    r->tag = f[3];
    r->pid = strtoull(f[4], 0, 10);
    r->uid = (unsigned)strtoul(f[5], 0, 10);
    r->msg = f[6];
    return 1;
}

// Every stored line, oldest first (system.log.4 ... .1, system.log); a
// NULL-terminated malloc'd array of malloc'd lines. NULL with errno set
// (EACCES: not root or in group adm) if the current file cannot be read.
static __attribute__((unused)) char **lf_load(int *count) {
    int n = 0, cap = 1024, err = 0;
    char **v = malloc(sizeof *v * (size_t)cap);
    if (!v) return 0;
    char *line = malloc(RX_LOG_TEXT_MAX + 256);
    for (int k = RX_LOG_KEEP; k >= 0; k--) {
        char path[64];
        if (k) snprintf(path, sizeof path, "%s.%d", RX_LOG_FILE, k);
        else snprintf(path, sizeof path, "%s", RX_LOG_FILE);
        FILE *f = fopen(path, "r");
        if (!f) {
            if (!k) err = errno;
            continue;
        }
        while (fgets(line, RX_LOG_TEXT_MAX + 256, f)) {
            if (n + 1 >= cap) {
                cap *= 2;
                char **nv = realloc(v, sizeof *v * (size_t)cap);
                if (!nv) break;
                v = nv;
            }
            v[n++] = strdup(line);
        }
        fclose(f);
    }
    free(line);
    v[n] = 0;
    if (!n && err) {
        free(v);
        errno = err;
        return 0;
    }
    if (count) *count = n;
    return v;
}

static const char *const lf_prio_names[8] = { "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug" };

#endif
