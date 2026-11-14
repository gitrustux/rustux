// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// logs (also journalctl): show the system log (step 18), what logd keeps
// in /var/log/system.log and the rotated files before it. Root and group
// adm may read it.
//
//   logs [-u UNIT]... [-k] [-b [N]] [-p PRIO] [-g TEXT] [--since T] [--until T]
//        [-n [N]] [-r] [-f] [-o short|short-iso|json|cat]
//   logs --list-boots | --disk-usage

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#include "logfile.h"

#define MAX_UNITS 16

static const char *units[MAX_UNITS];
static int nunits, kernel_only, reverse, follow, color;
static int boot_set;      // -b given
static long boot_arg;     // 0 current, -1 previous, N > 0 that boot
static unsigned boot_want;
static int max_prio = 7;
static const char *grep;
static long long since = -1, until = -1;
static enum { O_SHORT, O_ISO, O_JSON, O_CAT } out = O_SHORT;
static char host[65] = "rustux";

static int usage(int code) {
    fprintf(code ? stderr : stdout,
            "usage: logs [-u UNIT]... [-k] [-b [N]] [-p PRIO] [-g TEXT] [--since TIME] [--until TIME]\n"
            "            [-n [N]] [-r] [-f] [-o short|short-iso|json|cat]\n"
            "       logs --list-boots | --disk-usage\n"
            "Show the system log (see man logs). TIME: YYYY-MM-DD [HH:MM[:SS]], today, yesterday, now,\n"
            "-30s, -10min, -2h, -1d. PRIO: 0-7 or emerg alert crit err warning notice info debug.\n");
    return code;
}

// Milliseconds since 1970, local time as written, or -1
static long long parse_time(const char *s) {
    time_t now = time(0);
    if (!strcmp(s, "now")) return (long long)now * 1000;
    if (s[0] == '-' && s[1] >= '0' && s[1] <= '9') {
        char *e;
        long v = strtol(s + 1, &e, 10);
        long mul = !strcmp(e, "s") || !strcmp(e, "sec") ? 1 : !strcmp(e, "m") || !strcmp(e, "min") ? 60
                 : !strcmp(e, "h") ? 3600 : !strcmp(e, "d") ? 86400 : 0;
        if (!mul) return -1;
        return ((long long)now - v * mul) * 1000;
    }
    struct tm tm;
    localtime_r(&now, &tm);
    tm.tm_hour = tm.tm_min = tm.tm_sec = 0;
    tm.tm_isdst = -1;
    if (!strcmp(s, "today")) return (long long)mktime(&tm) * 1000;
    if (!strcmp(s, "yesterday")) {
        tm.tm_mday--;
        return (long long)mktime(&tm) * 1000;
    }
    int y, mo, d, h = 0, mi = 0, sec = 0;
    if (strlen(s) < 10 || s[4] != '-' || s[7] != '-') return -1;
    y = atoi(s); mo = atoi(s + 5); d = atoi(s + 8);
    if (s[10] == ' ' || s[10] == 'T') {
        if (strlen(s) < 16 || s[13] != ':') return -1;
        h = atoi(s + 11); mi = atoi(s + 14);
        if (s[16] == ':') sec = atoi(s + 17);
    } else if (s[10]) {
        return -1;
    }
    memset(&tm, 0, sizeof tm);
    tm.tm_year = y - 1900; tm.tm_mon = mo - 1; tm.tm_mday = d;
    tm.tm_hour = h; tm.tm_min = mi; tm.tm_sec = sec; tm.tm_isdst = -1;
    return (long long)mktime(&tm) * 1000;
}

static int parse_prio(const char *s) {
    if (s[0] >= '0' && s[0] <= '7' && !s[1]) return s[0] - '0';
    for (int i = 0; i < 8; i++) if (!strcmp(s, lf_prio_names[i])) return i;
    if (!strcmp(s, "error")) return 3;
    if (!strcmp(s, "warn")) return 4;
    if (!strcmp(s, "panic")) return 0;
    return -1;
}

static int matches(const logrec_t *r) {
    if (boot_set && r->boot != boot_want) return 0;
    if ((int)r->prio > max_prio) return 0;
    if (kernel_only && strcmp(r->tag, "kernel")) return 0;
    if (nunits) {
        int ok = 0;
        for (int i = 0; i < nunits; i++) {
            size_t l = strlen(units[i]);
            // "authd" and "authd.svc" both name it
            if (!strcmp(r->tag, units[i]) || (l > 4 && !strcmp(units[i] + l - 4, ".svc") && !strncmp(r->tag, units[i], l - 4) && !r->tag[l - 4]))
                ok = 1;
        }
        if (!ok) return 0;
    }
    if (since >= 0 && r->ms < since) return 0;
    if (until >= 0 && r->ms > until) return 0;
    if (grep && !strstr(r->msg, grep)) return 0;
    return 1;
}

static void json_str(const char *s) {
    putchar('"');
    for (; *s; s++) {
        unsigned char c = (unsigned char)*s;
        if (c == '"' || c == '\\') printf("\\%c", c);
        else if (c < 0x20) printf("\\u%04x", c);
        else putchar(c);
    }
    putchar('"');
}

static void show(const logrec_t *r) {
    if (out == O_JSON) {
        printf("{\"time\":\"%s\",\"boot\":%u,\"priority\":%u,\"unit\":", r->time, r->boot, r->prio);
        json_str(r->tag);
        printf(",\"pid\":%llu,\"uid\":%u,\"message\":", r->pid, r->uid);
        json_str(r->msg);
        printf("}\n");
        return;
    }
    const char *on = "", *off = "";
    if (color && r->prio <= 3) on = "\033[1;31m", off = "\033[0m";
    else if (color && r->prio == 4) on = "\033[33m", off = "\033[0m";
    if (out == O_CAT) {
        printf("%s%s%s\n", on, r->msg, off);
        return;
    }
    time_t t = (time_t)(r->ms / 1000);
    struct tm tm;
    localtime_r(&t, &tm);
    char when[40];
    if (out == O_ISO) {
        long off_s = tm.tm_gmtoff;
        snprintf(when, sizeof when, "%04d-%02d-%02dT%02d:%02d:%02d%c%02ld%02ld", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday,
                 tm.tm_hour, tm.tm_min, tm.tm_sec, off_s < 0 ? '-' : '+', labs(off_s) / 3600, labs(off_s) % 3600 / 60);
    } else {
        strftime(when, sizeof when, "%b %d %H:%M:%S", &tm);
    }
    if (r->pid) printf("%s %s %s[%llu]: %s%s%s\n", when, host, r->tag, r->pid, on, r->msg, off);
    else printf("%s %s %s: %s%s%s\n", when, host, r->tag, on, r->msg, off);
}

static void denied(void) {
    if (errno == EACCES) fprintf(stderr, "logs: %s: Permission denied (root and group adm may read the log)\n", RX_LOG_FILE);
    else if (errno == ENOENT) fprintf(stderr, "logs: no log yet (%s: is logd running?)\n", RX_LOG_FILE);
    else fprintf(stderr, "logs: %s: %s\n", RX_LOG_FILE, strerror(errno));
}

static unsigned current_boot(char **v) {
    unsigned b = 0;
    logrec_t r;
    for (int i = 0; v[i]; i++) {
        char tmp[64];
        // the boot is the second field: cheap to find without a full parse
        const char *t = strchr(v[i], '\t');
        if (!t) continue;
        snprintf(tmp, sizeof tmp, "%s", t + 1);
        unsigned x = (unsigned)strtoul(tmp, 0, 10);
        if (x > b) b = x;
    }
    (void)r;
    return b;
}

static int list_boots(char **v) {
    unsigned last = 0;
    long long first_ms = 0, last_ms = 0;
    int any = 0;
    unsigned cur = current_boot(v);
    printf("%5s %5s  %-19s  %-19s\n", "IDX", "BOOT", "FIRST", "LAST");
    for (int i = 0;; i++) {
        logrec_t r;
        char *copy = v[i] ? strdup(v[i]) : 0;
        int ok = copy && lf_parse(copy, &r);
        if (!v[i] || (ok && any && r.boot != last)) {
            if (any) {
                char a[24], b[24];
                time_t ta = (time_t)(first_ms / 1000), tb = (time_t)(last_ms / 1000);
                struct tm tm;
                localtime_r(&ta, &tm);
                strftime(a, sizeof a, "%Y-%m-%d %H:%M:%S", &tm);
                localtime_r(&tb, &tm);
                strftime(b, sizeof b, "%Y-%m-%d %H:%M:%S", &tm);
                printf("%5ld %5u  %-19s  %-19s\n", (long)last - (long)cur, last, a, b);
            }
            if (!v[i]) { free(copy); break; }
            any = 0;
        }
        if (ok) {
            if (!any) first_ms = r.ms;
            any = 1;
            last = r.boot;
            last_ms = r.ms;
        }
        free(copy);
    }
    return 0;
}

static int disk_usage(void) {
    long long total = 0;
    for (int k = 0; k <= RX_LOG_KEEP; k++) {
        char path[64];
        struct stat st;
        if (k) snprintf(path, sizeof path, "%s.%d", RX_LOG_FILE, k);
        else snprintf(path, sizeof path, "%s", RX_LOG_FILE);
        if (stat(path, &st) == 0) total += st.st_size;
    }
    if (total >= 1024 * 1024) printf("The logs take up %.1fM in /var/log.\n", (double)total / (1024 * 1024));
    else printf("The logs take up %.1fK in /var/log.\n", (double)total / 1024);
    return 0;
}

static int is_num(const char *s) {
    if (*s == '-' || *s == '+') s++;
    if (!*s) return 0;
    for (; *s; s++) if (*s < '0' || *s > '9') return 0;
    return 1;
}

int main(int argc, char **argv) {
    long lines = -1;
    int boots = 0, usage_only = 0;
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        const char *val = 0;
        // --opt=value
        char opt[32];
        if (a[0] == '-' && a[1] == '-' && strchr(a, '=')) {
            size_t l = (size_t)(strchr(a, '=') - a);
            if (l >= sizeof opt) return usage(2);
            memcpy(opt, a, l);
            opt[l] = 0;
            val = a + l + 1;
            a = opt;
        }
#define ARG() (val ? val : (i + 1 < argc ? argv[++i] : (fprintf(stderr, "logs: %s needs a value\n", a), exit(2), "")))
        if (!strcmp(a, "-u") || !strcmp(a, "--unit") || !strcmp(a, "-t") || !strcmp(a, "--identifier")) {
            if (nunits < MAX_UNITS) units[nunits++] = ARG();
        } else if (!strcmp(a, "-k") || !strcmp(a, "--dmesg")) {
            kernel_only = 1;
            if (!boot_set) boot_set = 1, boot_arg = 0;
        } else if (!strcmp(a, "-b") || !strcmp(a, "--boot")) {
            boot_set = 1;
            boot_arg = 0;
            if (val) boot_arg = strtol(val, 0, 10);
            else if (i + 1 < argc && is_num(argv[i + 1])) boot_arg = strtol(argv[++i], 0, 10);
        } else if (!strcmp(a, "-p") || !strcmp(a, "--priority")) {
            const char *p = ARG();
            if ((max_prio = parse_prio(p)) < 0) { fprintf(stderr, "logs: bad priority %s\n", p); return 2; }
        } else if (!strcmp(a, "-g") || !strcmp(a, "--grep")) {
            grep = ARG();
        } else if (!strcmp(a, "-S") || !strcmp(a, "--since") || !strcmp(a, "-U") || !strcmp(a, "--until")) {
            const char *t = ARG();
            long long ms = parse_time(t);
            if (ms < 0) { fprintf(stderr, "logs: bad time %s\n", t); return 2; }
            if (a[1] == 'S' || a[2] == 's') since = ms;
            else until = ms + 999;
        } else if (!strcmp(a, "-n") || !strcmp(a, "--lines")) {
            lines = 10;
            if (val) lines = strtol(val, 0, 10);
            else if (i + 1 < argc && is_num(argv[i + 1])) lines = strtol(argv[++i], 0, 10);
        } else if (!strcmp(a, "-r") || !strcmp(a, "--reverse")) {
            reverse = 1;
        } else if (!strcmp(a, "-f") || !strcmp(a, "--follow")) {
            follow = 1;
        } else if (!strcmp(a, "-o") || !strcmp(a, "--output")) {
            const char *o = ARG();
            if (!strcmp(o, "short")) out = O_SHORT;
            else if (!strcmp(o, "short-iso")) out = O_ISO;
            else if (!strcmp(o, "json")) out = O_JSON;
            else if (!strcmp(o, "cat")) out = O_CAT;
            else { fprintf(stderr, "logs: unknown output %s (short, short-iso, json, cat)\n", o); return 2; }
        } else if (!strcmp(a, "--list-boots")) {
            boots = 1;
        } else if (!strcmp(a, "--disk-usage")) {
            usage_only = 1;
        } else if (!strcmp(a, "--no-pager") || !strcmp(a, "-a") || !strcmp(a, "--all") || !strcmp(a, "-q") || !strcmp(a, "--quiet") || !strcmp(a, "-x")) {
            // nothing to do: there is no pager, nothing is cut short
        } else if (!strcmp(a, "-h") || !strcmp(a, "--help")) {
            return usage(0);
        } else {
            fprintf(stderr, "logs: unknown option %s\n", a);
            return usage(2);
        }
    }
    if (usage_only) return disk_usage();
    gethostname(host, sizeof host);
    color = isatty(1) && out != O_JSON;
    int n = 0;
    char **v = lf_load(&n);
    if (!v) {
        denied();
        return 1;
    }
    if (boots) return list_boots(v);
    unsigned cur = current_boot(v);
    if (boot_set) boot_want = boot_arg > 0 ? (unsigned)boot_arg : (unsigned)((long)cur + boot_arg);
    if (follow && lines < 0) lines = 10;
    // The matches, then the last `lines` of them
    int *idx = malloc(sizeof *idx * (size_t)(n + 1)), m = 0;
    logrec_t *recs = malloc(sizeof *recs * (size_t)(n + 1));
    if (!idx || !recs) return 1;
    for (int i = 0; i < n; i++) {
        if (!lf_parse(v[i], &recs[i])) continue;
        if (matches(&recs[i])) idx[m++] = i;
    }
    int from = lines >= 0 && m > lines ? m - (int)lines : 0;
    if (reverse) for (int k = m - 1; k >= from; k--) show(&recs[idx[k]]);
    else for (int k = from; k < m; k++) show(&recs[idx[k]]);
    fflush(stdout);
    if (!follow) return 0;
    // Follow the current file: new lines as they come; a rotation starts it over
    struct stat st;
    long long pos = stat(RX_LOG_FILE, &st) == 0 ? (long long)st.st_size : 0;
    char part[RX_LOG_TEXT_MAX + 256];
    size_t plen = 0;
    for (;;) {
        usleep(250000);
        if (stat(RX_LOG_FILE, &st) < 0) continue;
        if ((long long)st.st_size < pos) pos = 0, plen = 0;
        if ((long long)st.st_size == pos) continue;
        int fd = open(RX_LOG_FILE, O_RDONLY);
        if (fd < 0) continue;
        lseek(fd, (off_t)pos, SEEK_SET);
        char buf[4096];
        ssize_t got;
        while ((got = read(fd, buf, sizeof buf)) > 0) {
            pos += got;
            for (ssize_t i = 0; i < got; i++) {
                if (plen < sizeof part - 1) part[plen++] = buf[i];
                if (buf[i] != '\n') continue;
                part[plen] = 0;
                logrec_t r;
                if (lf_parse(part, &r) && matches(&r)) show(&r);
                plen = 0;
            }
        }
        close(fd);
        fflush(stdout);
    }
}
