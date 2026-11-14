// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// svc (also systemctl): ask init's service manager (step 18) about units
// and tell it what to start and stop. Units are /etc/svc/NAME.svc
// (docs/SERVICES.md). Looking is for anyone; changing is root's (sudo).
//
//   svc [list]                      every unit (systemctl: list-units, list-unit-files)
//   svc status [UNIT]               one unit and its last log lines; without UNIT a summary
//   svc start|stop|restart UNIT...
//   svc enable|disable [--now] UNIT...
//   svc reload                      read /etc/svc again (systemctl: daemon-reload)
//   svc reset-failed [UNIT]
//   svc isolate rescue|multi-user   switch targets
//   svc is-active|is-enabled|is-failed UNIT
//   svc cat UNIT | show UNIT | get-default
//   svc poweroff | reboot
//   --json (-o json, --output=json): JSON instead of tables

#include <errno.h>
#include <rustux/auth.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "logfile.h"
#include "sysreq.h"

static const char *prog = "svc";
static int json;

typedef struct {
    char *f[RX_SVC_LIST_FIELDS];
} row_t;
enum { F_NAME, F_STATE, F_ENABLED, F_PID, F_SINCE, F_STARTS, F_CODE, F_TARGETS, F_USER, F_TYPE, F_RESTART, F_EXEC, F_DESC, F_REASON };

static char reply[65536];
static row_t rows[80];
static int nrows;
static char target[32] = "?";

// One request; the reply's text in `reply`; the status (0 or -errno)
static int ask(uint32_t cmd, const char *name) {
    rx_handle_t ch;
    int r = rx_service_connect("/svc/system", &ch);
    if (r < 0) {
        fprintf(stderr, "%s: cannot reach the service manager (/svc/system): %s\n", prog, strerror(-r));
        exit(1);
    }
    rx_svc_req_t rq;
    memset(&rq, 0, sizeof rq);
    rq.op = RX_SYSTEM_OP_SVC;
    rq.cmd = cmd;
    snprintf(rq.name, sizeof rq.name, "%s", name ? name : "");
    if (rx_channel_write(ch, 0, &rq, sizeof rq, 0, 0) != RX_OK) {
        fprintf(stderr, "%s: cannot reach the service manager\n", prog);
        exit(1);
    }
    uint32_t nb = 0;
    for (;;) {
        rx_status_t s = rx_channel_read(ch, 0, reply, 0, sizeof reply - 1, 0, &nb, 0);
        if (s == RX_ERR_SHOULD_WAIT) {
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
            continue;
        }
        if (s != RX_OK || nb < 4) {
            fprintf(stderr, "%s: the service manager did not answer\n", prog);
            exit(1);
        }
        break;
    }
    rx_handle_close(ch);
    int32_t st;
    memcpy(&st, reply, 4);
    memmove(reply, reply + 4, nb - 4);
    reply[nb - 4] = 0;
    return st;
}

// The reply of LIST or STATUS as rows
static void parse_rows(void) {
    nrows = 0;
    char *save = 0;
    for (char *line = strtok_r(reply, "\n", &save); line; line = strtok_r(0, "\n", &save)) {
        if (!strncmp(line, "target\t", 7)) {
            snprintf(target, sizeof target, "%s", line + 7);
            continue;
        }
        if (nrows == (int)(sizeof rows / sizeof rows[0])) break;
        row_t *r = &rows[nrows];
        int k = 0;
        r->f[k++] = line;
        for (char *p = line; *p && k < RX_SVC_LIST_FIELDS; p++)
            if (*p == '\t') {
                *p = 0;
                r->f[k++] = p + 1;
            }
        if (k == RX_SVC_LIST_FIELDS) nrows++;
    }
}

static void when(const char *ms_s, char *out, size_t cap, int with_ago) {
    long long ms = atoll(ms_s);
    if (ms <= 0) {
        snprintf(out, cap, "-");
        return;
    }
    time_t t = (time_t)(ms / 1000), now = time(0);
    struct tm tm;
    localtime_r(&t, &tm);
    char a[32];
    strftime(a, sizeof a, now - t < 86400 ? "%H:%M:%S" : "%Y-%m-%d %H:%M", &tm);
    if (!with_ago) {
        snprintf(out, cap, "%s", a);
        return;
    }
    long d = (long)(now - t);
    if (d < 0) d = 0;
    if (d < 60) snprintf(out, cap, "%s (%lds ago)", a, d);
    else if (d < 3600) snprintf(out, cap, "%s (%ldmin ago)", a, d / 60);
    else if (d < 86400) snprintf(out, cap, "%s (%ldh %ldmin ago)", a, d / 3600, d % 3600 / 60);
    else snprintf(out, cap, "%s (%ld days ago)", a, d / 86400);
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

static void json_row(const row_t *r) {
    static const char *const keys[RX_SVC_LIST_FIELDS] = { "unit", "state", "enabled", "pid", "since", "starts", "last_exit",
                                                          "targets", "user", "type", "restart", "exec", "description", "reason" };
    printf("{");
    for (int k = 0; k < RX_SVC_LIST_FIELDS; k++) {
        printf("%s\"%s\":", k ? "," : "", keys[k]);
        const char *v = r->f[k];
        int num = k == F_PID || k == F_STARTS || k == F_SINCE;
        if (k == F_CODE && strcmp(v, "-")) num = 1;
        if (num) printf("%s", v);
        else if (!strcmp(v, "-") && (k == F_CODE || k == F_REASON || k == F_DESC)) printf("null");
        else json_str(v);
    }
    printf("}");
}

static const char *color_of(const char *state) {
    if (!isatty(1)) return "";
    if (!strcmp(state, "running") || !strcmp(state, "exited")) return "\033[32m";
    if (!strcmp(state, "failed")) return "\033[1;31m";
    if (!strcmp(state, "restarting") || !strcmp(state, "starting") || !strcmp(state, "waiting") || !strcmp(state, "stopping")) return "\033[33m";
    return "";
}

static int cmd_list(void) {
    ask(RX_SVC_LIST, 0);
    parse_rows();
    if (json) {
        printf("{\"target\":");
        json_str(target);
        printf(",\"units\":[");
        for (int i = 0; i < nrows; i++) {
            if (i) printf(",");
            json_row(&rows[i]);
        }
        printf("]}\n");
        return 0;
    }
    printf("%-14s %-10s %-9s %6s %-9s %s\n", "UNIT", "STATE", "ENABLED", "PID", "SINCE", "DESCRIPTION");
    int failed = 0;
    for (int i = 0; i < nrows; i++) {
        row_t *r = &rows[i];
        char since[40];
        when(r->f[F_SINCE], since, sizeof since, 0);
        const char *c = color_of(r->f[F_STATE]);
        printf("%-14s %s%-10s%s %-9s %6s %-9s %s\n", r->f[F_NAME], c, r->f[F_STATE], *c ? "\033[0m" : "", r->f[F_ENABLED],
               strcmp(r->f[F_PID], "0") ? r->f[F_PID] : "-", since, strcmp(r->f[F_DESC], "-") ? r->f[F_DESC] : "");
        if (!strcmp(r->f[F_STATE], "failed")) failed++;
    }
    printf("\nTarget %s; %d units%s", target, nrows, failed ? "" : ".\n");
    if (failed) printf(", %d failed.\n", failed);
    return 0;
}

static void log_tail(const char *unit, int n) {
    int count = 0;
    char **v = lf_load(&count);
    if (!v) return; // not root or in group adm: no log lines
    int m = 0, *idx = malloc(sizeof *idx * (size_t)(count + 1));
    logrec_t *recs = malloc(sizeof *recs * (size_t)(count + 1));
    if (!idx || !recs) return;
    for (int i = 0; i < count; i++) {
        if (!lf_parse(v[i], &recs[i])) continue;
        int mine = !strcmp(recs[i].tag, unit);
        // init's lines about it: "started NAME", "NAME exited ..."
        if (!mine && !strcmp(recs[i].tag, "init")) {
            const char *s = strstr(recs[i].msg, unit);
            size_t l = strlen(unit);
            mine = s && (s == recs[i].msg || s[-1] == ' ') && (s[l] == ' ' || s[l] == ':' || !s[l]);
        }
        if (mine) idx[m++] = i;
    }
    if (!m) return;
    printf("\n");
    for (int k = m > n ? m - n : 0; k < m; k++) {
        logrec_t *r = &recs[idx[k]];
        time_t t = (time_t)(r->ms / 1000);
        struct tm tm;
        localtime_r(&t, &tm);
        char a[24];
        strftime(a, sizeof a, "%b %d %H:%M:%S", &tm);
        if (r->pid) printf("%s %s[%llu]: %s\n", a, r->tag, r->pid, r->msg);
        else printf("%s %s: %s\n", a, r->tag, r->msg);
    }
}

static int cmd_status(const char *name) {
    if (!name) {
        ask(RX_SVC_LIST, 0);
        parse_rows();
        int running = 0, failed = 0;
        for (int i = 0; i < nrows; i++) {
            if (!strcmp(rows[i].f[F_STATE], "running")) running++;
            if (!strcmp(rows[i].f[F_STATE], "failed")) failed++;
        }
        if (json) {
            printf("{\"target\":");
            json_str(target);
            printf(",\"units\":%d,\"running\":%d,\"failed\":%d}\n", nrows, running, failed);
            return failed ? 1 : 0;
        }
        char host[65] = "rustux";
        gethostname(host, sizeof host);
        printf("%s\n    Target: %s\n     State: %s\n     Units: %d, %d running, %d failed\n", host, target,
               failed ? "degraded" : "running", nrows, running, failed);
        for (int i = 0; i < nrows; i++)
            if (!strcmp(rows[i].f[F_STATE], "failed")) printf("    Failed: %s: %s\n", rows[i].f[F_NAME], rows[i].f[F_REASON]);
        return failed ? 1 : 0;
    }
    int st = ask(RX_SVC_STATUS, name);
    if (st < 0) {
        fprintf(stderr, "%s", reply[0] ? reply : "no such unit\n");
        return 4;
    }
    parse_rows();
    if (!nrows) return 4;
    row_t *r = &rows[0];
    if (json) {
        json_row(r);
        printf("\n");
    } else {
        const char *c = color_of(r->f[F_STATE]);
        char since[48];
        when(r->f[F_SINCE], since, sizeof since, 1);
        printf("%s* %s%s - %s\n", c, r->f[F_NAME], *c ? "\033[0m" : "", strcmp(r->f[F_DESC], "-") ? r->f[F_DESC] : "");
        printf("     State: %s%s%s since %s", c, r->f[F_STATE], *c ? "\033[0m" : "", since);
        if (strcmp(r->f[F_PID], "0")) printf(", pid %s", r->f[F_PID]);
        printf("\n");
        if (strcmp(r->f[F_REASON], "-")) printf("    Reason: %s\n", r->f[F_REASON]);
        printf("   Enabled: %s (targets: %s)\n", r->f[F_ENABLED], r->f[F_TARGETS]);
        printf("      Exec: %s\n", r->f[F_EXEC]);
        printf("      User: %s   Type: %s   Restart: %s\n", r->f[F_USER], r->f[F_TYPE], r->f[F_RESTART]);
        printf("    Starts: %s   Last exit: %s\n", r->f[F_STARTS], r->f[F_CODE]);
        char unit[64];
        snprintf(unit, sizeof unit, "%s", r->f[F_NAME]);
        log_tail(unit, 10);
    }
    return !strcmp(r->f[F_STATE], "running") || !strcmp(r->f[F_STATE], "exited") ? 0 : 3;
}

static const char *state_of(const char *name, char *buf, size_t cap, int field) {
    if (ask(RX_SVC_STATUS, name) < 0) return 0;
    parse_rows();
    if (!nrows) return 0;
    snprintf(buf, cap, "%s", rows[0].f[field]);
    return buf;
}

// After start/restart: wait (up to 5 s) until it is up (a oneshot: has
// finished) or has failed
static int wait_up(const char *name) {
    char st[32];
    for (int i = 0; i < 50; i++) {
        if (!state_of(name, st, sizeof st, F_STATE)) return 1;
        // a oneshot is done when it has exited
        int oneshot = nrows && !strcmp(rows[0].f[F_TYPE], "oneshot");
        if ((!strcmp(st, "running") && !oneshot) || !strcmp(st, "exited")) return 0;
        if (!strcmp(st, "failed") || !strcmp(st, "inactive")) {
            char why[100] = "";
            if (nrows) snprintf(why, sizeof why, "%s", rows[0].f[F_REASON]);
            fprintf(stderr, "%s: %s %s: %s (svc status %s, logs -u %s)\n", prog, name, st, why, name, name);
            return 1;
        }
        usleep(100000);
    }
    return 0; // still on its way (waiting for what it comes after, or restarting)
}

static int simple(uint32_t cmd, const char *name) {
    int st = ask(cmd, name);
    if (reply[0]) fputs(reply, st < 0 ? stderr : stdout);
    if (st == -EPERM) fprintf(stderr, "%s: only root may do that (try sudo %s)\n", prog, prog);
    else if (st < 0 && !reply[0]) fprintf(stderr, "%s: %s\n", prog, strerror(-st));
    return st < 0 ? 1 : 0;
}

static int cmd_cat(const char *name) {
    char path[128];
    size_t l = strlen(name);
    snprintf(path, sizeof path, "/etc/svc/%s%s", name, l > 4 && !strcmp(name + l - 4, ".svc") ? "" : ".svc");
    FILE *f = fopen(path, "r");
    if (!f) {
        char st[32];
        if (state_of(name, st, sizeof st, F_ENABLED) && (!strcmp(st, "builtin") || !strcmp(st, "static"))) {
            printf("# %s is built into init (there is no %s)\n", name, path);
            return 0;
        }
        fprintf(stderr, "%s: %s: %s\n", prog, path, strerror(errno));
        return 1;
    }
    printf("# %s\n", path);
    char line[600];
    while (fgets(line, sizeof line, f)) fputs(line, stdout);
    fclose(f);
    return 0;
}

static int cmd_show(const char *name) {
    if (ask(RX_SVC_STATUS, name) < 0) {
        fprintf(stderr, "%s", reply);
        return 1;
    }
    parse_rows();
    if (!nrows) return 1;
    row_t *r = &rows[0];
    static const char *const keys[RX_SVC_LIST_FIELDS] = { "Id", "State", "Enabled", "MainPID", "Since", "Starts", "LastExit",
                                                          "Targets", "User", "Type", "Restart", "Exec", "Description", "Reason" };
    for (int k = 0; k < RX_SVC_LIST_FIELDS; k++) printf("%s=%s\n", keys[k], r->f[k]);
    return 0;
}

static int usage(int code) {
    fprintf(code ? stderr : stdout,
            "usage: %s [list | status [UNIT] | start|stop|restart UNIT... | enable|disable [--now] UNIT...\n"
            "            | reload | reset-failed [UNIT] | isolate rescue|multi-user\n"
            "            | is-active|is-enabled|is-failed UNIT | cat UNIT | show UNIT | get-default\n"
            "            | poweroff | reboot] [--json]\n"
            "Units are /etc/svc/NAME.svc (see man svc). Changes need root (sudo).\n",
            prog);
    return code;
}

int main(int argc, char **argv) {
    const char *base = strrchr(argv[0], '/') ? strrchr(argv[0], '/') + 1 : argv[0];
    if (!strcmp(base, "systemctl")) prog = "systemctl";
    char *args[64];
    int na = 0, now = 0;
    for (int i = 1; i < argc && na < 63; i++) {
        const char *a = argv[i];
        if (!strcmp(a, "--json") || !strcmp(a, "--output=json") || !strcmp(a, "-ojson")) json = 1;
        else if ((!strcmp(a, "-o") || !strcmp(a, "--output")) && i + 1 < argc) json = !strcmp(argv[++i], "json");
        else if (!strcmp(a, "--now")) now = 1;
        else if (!strcmp(a, "--no-pager") || !strcmp(a, "-a") || !strcmp(a, "--all") || !strcmp(a, "-l") || !strcmp(a, "--full") || !strcmp(a, "-q") || !strcmp(a, "--quiet")) {}
        else if (!strcmp(a, "-h") || !strcmp(a, "--help")) return usage(0);
        else if (a[0] == '-' && a[1]) { fprintf(stderr, "%s: unknown option %s\n", prog, a); return usage(2); }
        else args[na++] = argv[i];
    }
    args[na] = 0;
    const char *cmd = na ? args[0] : "list";
    char **names = args + 1;
    int nn = na ? na - 1 : 0;
    if (!strcmp(cmd, "list") || !strcmp(cmd, "list-units") || !strcmp(cmd, "list-unit-files")) return cmd_list();
    if (!strcmp(cmd, "status")) {
        if (nn <= 1) return cmd_status(nn ? names[0] : 0);
        int rc = 0;
        for (int i = 0; i < nn; i++) {
            if (i) printf("\n");
            int r = cmd_status(names[i]);
            if (r) rc = r;
        }
        return rc;
    }
    // reload alone reads /etc/svc again; reload UNIT restarts it (there is no reload signal)
    if ((!strcmp(cmd, "reload") && !nn) || !strcmp(cmd, "daemon-reload")) return simple(RX_SVC_RELOAD, 0);
    if (!strcmp(cmd, "reload")) cmd = "restart";
    if (!strcmp(cmd, "get-default") || !strcmp(cmd, "target")) {
        ask(RX_SVC_LIST, 0);
        parse_rows();
        printf("%s\n", target);
        return 0;
    }
    if (!strcmp(cmd, "isolate")) {
        if (nn != 1) return usage(2);
        return simple(RX_SVC_ISOLATE, names[0]);
    }
    if (!strcmp(cmd, "reset-failed")) {
        if (!nn) return simple(RX_SVC_RESET_FAILED, "");
        int rc = 0;
        for (int i = 0; i < nn; i++) rc |= simple(RX_SVC_RESET_FAILED, names[i]);
        return rc;
    }
    if (!strcmp(cmd, "poweroff") || !strcmp(cmd, "reboot") || !strcmp(cmd, "halt")) {
        int r = sysreq(!strcmp(cmd, "reboot") ? RX_SYSTEM_OP_REBOOT : RX_SYSTEM_OP_POWEROFF, 0, 0);
        fprintf(stderr, "%s: %s: %s\n", prog, cmd, r == -EPERM ? "only root may do that (try sudo)" : strerror(-r));
        return 1;
    }
    if (!nn) return usage(2);
    int rc = 0;
    for (int i = 0; i < nn; i++) {
        const char *n = names[i];
        if (!strcmp(cmd, "start")) rc |= simple(RX_SVC_START, n) || wait_up(n);
        else if (!strcmp(cmd, "stop")) rc |= simple(RX_SVC_STOP, n);
        else if (!strcmp(cmd, "restart") || !strcmp(cmd, "try-restart") || !strcmp(cmd, "reload-or-restart"))
            rc |= simple(RX_SVC_RESTART, n) || wait_up(n);
        else if (!strcmp(cmd, "enable")) rc |= simple(RX_SVC_ENABLE, n) || (now && (simple(RX_SVC_START, n) || wait_up(n)));
        else if (!strcmp(cmd, "disable")) rc |= simple(RX_SVC_DISABLE, n) || (now && simple(RX_SVC_STOP, n));
        else if (!strcmp(cmd, "is-active") || !strcmp(cmd, "is-failed")) {
            char st[32];
            if (!state_of(n, st, sizeof st, F_STATE)) {
                printf("unknown\n");
                rc = 1;
                continue;
            }
            printf("%s\n", st);
            int active = !strcmp(st, "running") || !strcmp(st, "exited");
            if (cmd[3] == 'f' ? strcmp(st, "failed") : !active) rc = 1;
        } else if (!strcmp(cmd, "is-enabled")) {
            char st[32];
            if (!state_of(n, st, sizeof st, F_ENABLED)) {
                printf("unknown\n");
                rc = 1;
                continue;
            }
            printf("%s\n", st);
            if (!strcmp(st, "disabled")) rc = 1;
        } else if (!strcmp(cmd, "cat")) rc |= cmd_cat(n);
        else if (!strcmp(cmd, "show")) rc |= cmd_show(n);
        else {
            fprintf(stderr, "%s: unknown command %s\n", prog, cmd);
            return usage(2);
        }
    }
    return rc;
}
