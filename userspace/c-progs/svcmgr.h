// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// svcmgr.h: init's service manager (step 18), included by init.c (it uses
// init's console: console_copy(), console_output(), sh_ctl, autologin).
// docs/SERVICES.md has the whole story; in short:
//
// - A unit is /etc/svc/NAME.svc: key=value lines (exec, user, type,
//   restart, needs, after, target, ...). It is enabled when
//   /etc/svc/enabled/NAME exists. Without /etc/svc (a disk from before
//   step 18) the same units are built in.
// - At boot the enabled units of the target (multi-user, or rescue from
//   the kernel command line) start in dependency order: `needs` pulls a
//   unit in and fails with it, `after` only orders. A unit with
//   `ready=/svc/NAME` is up once that service answers.
// - Each unit runs as its user in a job of its own (ended with it); its
//   stdout and stderr go to logd (log=console: to the screen). When it
//   ends, `restart` decides; more than `start_limit` starts in 30 s fail it.
// - One unit holds the console (console=yes): login, or the rescue shell.
//   If none can, init starts the emergency unit, a root shell.
// - svc (systemctl) asks over /svc/system (RX_SYSTEM_OP_SVC).

#ifndef SVCMGR_H
#define SVCMGR_H

#include <dirent.h>
#include <rustux/log.h>
#include <stdarg.h>
#include <sys/stat.h>
#include <syslog.h> // the LOG_ priorities

#include "userdb.h"

#define MAX_UNITS 64
#define TGT_MULTI  1
#define TGT_RESCUE 2
#define TGT_INSTALL 4  // step 20: the installer on the console
#define TGT_GRAPHICAL 8  // DESKTOP.md step F6: multi-user and the desktop

// A unit belongs to target t (graphical takes everything multi-user has)
static int in_target(int targets, int t) { return (targets & t) || (t == TGT_GRAPHICAL && (targets & TGT_MULTI)); }
#define SVC_DIR "/etc/svc"
// Units that come with the system image (package step E6): read-only,
// new ones reach installed systems with each image. A file of the same
// name in /etc/svc replaces one; `enabled=yes` in it enables it unless
// /etc/svc/disabled/NAME exists (svc disable makes that)
#define VENDOR_DIR "/usr/lib/svc"
#define START_WINDOW_NS 30000000000ll
#define READY_TIMEOUT_NS 10000000000ll

enum { U_INACTIVE, U_WAITING, U_STARTING, U_RUNNING, U_EXITED, U_FAILED, U_STOPPING, U_RESTART_WAIT };
static const char *const ustate_names[] = { "inactive", "waiting", "starting", "running", "exited", "failed", "stopping", "restarting" };
enum { T_SIMPLE, T_ONESHOT };
enum { R_NO, R_ON_FAILURE, R_ALWAYS };

typedef struct {
    char name[32], desc[96], exec[256], user[32], needs[160], after[160], env[512], dir[128], ready[64];
    int type, restart, restart_ms, start_limit, targets, console, system_job, log_console, keep_processes;
    int enabled_default; // enabled=yes (units of /usr/lib/svc)
} ucfg_t;

typedef struct {
    ucfg_t c;
    int present, builtin, enabled, vendor;
    int state, restart_pending;
    rx_handle_t proc, job;
    rx_handle_t kept_job;  // keep_processes: the job of an ended run, left alone
    uint64_t pid;
    int64_t since_ms, last_code;
    int64_t deadline;      // monotonic ns: the restart (RESTART_WAIT), giving up on ready (STARTING)
    int have_code, starts, order;
    int64_t recent[8];     // monotonic ns of the last starts
    int nrecent;
    char reason[96];
} unit_t;

static unit_t units[MAX_UNITS];
static int nunits, start_counter, svc_target = TGT_MULTI, svc_shutting_down, svc_from_files;
static rx_handle_t log_ch; // init's connection to logd, for the services' output

static int64_t ms_now(void) { return rx_clock_get_utc() / 1000000; }
static int64_t mono_ns(void) { return (int64_t)rx_clock_get_monotonic(); }

// Built in: used when there is no /etc/svc (the files in the image are the same)
static const char *const builtin_units[][2] = {
    { "logd", "description=System log (/svc/log)\nexec=/bin/logd\nready=/svc/log\nrestart=always\nlog=console\ntarget=multi-user rescue install\n" },
    { "authd", "description=Authentication: su, sudo, passwd (/svc/auth)\nexec=/bin/authd\nafter=logd\nrestart=always\nsystem_job=yes\ntarget=multi-user\n" },
    { "mounts", "description=Mount the filesystems in /etc/fstab\ntype=oneshot\nexec=/bin/mount -a\nafter=logd\nkeep_processes=yes\ntarget=multi-user rescue install\n" },
    { "console", "description=Login on the console\nexec=/bin/login\nafter=logd authd\nconsole=yes\nrestart=always\nrestart_delay=0\nstart_limit=0\ntarget=multi-user\n" },
    { "rescue-shell", "description=Root shell on the console (rescue mode)\nexec=/bin/shell\nafter=logd\nconsole=yes\nrestart=always\nrestart_delay=0\nstart_limit=0\ntarget=rescue\n" },
    { "installer", "description=The installer on the console (install mode)\nexec=/bin/rustica-install\nafter=logd mounts\nconsole=yes\nsystem_job=yes\ntarget=install\n" },
};
// Never in a file: what init falls back to when nothing else holds the console
// Built in too (package step E4): once a multi-user or graphical boot has
// started everything, this boot is good for the boot manager: rboot's
// boot counter on the entry goes (bootctl bless), so a new image that gets
// this far is kept; one that does not is tried again, then rboot falls
// back to the previous image
static const char boot_complete_unit[] = "description=This boot is good: rboot's boot counter goes (bootctl bless)\nexec=/bin/bootctl bless\ntype=oneshot\ntarget=\n";
static int boot_complete_done;
static const char emergency_unit[] = "description=Root shell when nothing else runs on the console\nexec=/bin/shell\nconsole=yes\ntarget=\n";

static int log_connect(void) {
    if (!log_ch && rx_service_connect("/svc/log", &log_ch) < 0) log_ch = 0;
    return log_ch != 0;
}

// init's own messages (tag "init"). Until logd is up they wait here (the
// last 32), so the log shows the whole boot.
#define LOG_BACKLOG 32
typedef struct {
    rx_log_msg_t h;
    char text[200];
} ulog_msg_t;
static ulog_msg_t backlog[LOG_BACKLOG];
static int nbacklog;

static int ulog_send(const ulog_msg_t *m) {
    for (int tries = 0; tries < 2; tries++) {
        if (!log_connect()) return 0;
        if (rx_channel_write(log_ch, 0, m, (uint32_t)(sizeof m->h + strlen(m->text)), 0, 0) == RX_OK) return 1;
        rx_handle_close(log_ch); // logd has restarted: a new connection
        log_ch = 0;
    }
    return 0;
}

static void ulog(int prio, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
static void ulog(int prio, const char *fmt, ...) {
    ulog_msg_t m;
    memset(&m.h, 0, sizeof m.h);
    m.h.op = RX_LOG_OP_WRITE;
    m.h.priority = (uint32_t)prio;
    snprintf(m.h.tag, sizeof m.h.tag, "init");
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(m.text, sizeof m.text, fmt, ap);
    va_end(ap);
    int k = 0;
    while (k < nbacklog && ulog_send(&backlog[k])) k++;
    if (k) {
        memmove(backlog, backlog + k, sizeof backlog[0] * (size_t)(nbacklog - k));
        nbacklog -= k;
    }
    if (nbacklog || !ulog_send(&m)) {
        if (nbacklog == LOG_BACKLOG) memmove(backlog, backlog + 1, sizeof backlog[0] * (LOG_BACKLOG - 1)), nbacklog--;
        backlog[nbacklog++] = m;
    }
}

static unit_t *find_unit(const char *name) {
    size_t l = strlen(name);
    for (int i = 0; i < nunits; i++) {
        if (!strcmp(units[i].c.name, name)) return &units[i];
        // "authd.svc" names authd too
        if (l > 4 && !strcmp(name + l - 4, ".svc") && strlen(units[i].c.name) == l - 4 && !strncmp(units[i].c.name, name, l - 4))
            return &units[i];
    }
    return 0;
}

static void cfg_set(char *dst, size_t cap, const char *v) { snprintf(dst, cap, "%s", v); }

static int parse_ms(const char *v) {
    // seconds, with up to three decimals ("0.5")
    long s = strtol(v, 0, 10), frac = 0, scale = 100;
    const char *dot = strchr(v, '.');
    if (dot) for (const char *p = dot + 1; *p >= '0' && *p <= '9' && scale; p++, scale /= 10) frac += (*p - '0') * scale;
    return (int)(s * 1000 + frac);
}

// key=value lines; problems are logged (prefixed with where they are)
static void unit_parse(ucfg_t *c, const char *name, const char *text, const char *where) {
    memset(c, 0, sizeof *c);
    cfg_set(c->name, sizeof c->name, name);
    c->restart_ms = 1000;
    c->start_limit = 5;
    c->targets = TGT_MULTI;
    const char *p = text;
    int lineno = 0;
    while (*p) {
        char line[600];
        size_t n = strcspn(p, "\n");
        snprintf(line, sizeof line, "%.*s", (int)(n < sizeof line - 1 ? n : sizeof line - 1), p);
        p += n + (p[n] == '\n');
        lineno++;
        char *s = line;
        while (*s == ' ' || *s == '\t') s++;
        size_t e = strlen(s);
        while (e && (s[e - 1] == ' ' || s[e - 1] == '\t' || s[e - 1] == '\r')) s[--e] = 0;
        if (!*s || *s == '#' || *s == ';' || *s == '[') continue; // [Section] lines are allowed and ignored
        char *eq = strchr(s, '=');
        if (!eq) {
            ulog(LOG_WARNING, "%s:%d: not key=value: %s", where, lineno, s);
            continue;
        }
        *eq = 0;
        char *k = s, *v = eq + 1;
        for (char *t = eq - 1; t >= k && (*t == ' ' || *t == '\t'); t--) *t = 0;
        while (*v == ' ' || *v == '\t') v++;
        for (char *t = k; *t; t++) if (*t >= 'A' && *t <= 'Z') *t += 32;
        int yes = !strcmp(v, "yes") || !strcmp(v, "true") || !strcmp(v, "1");
        if (!strcmp(k, "description")) cfg_set(c->desc, sizeof c->desc, v);
        else if (!strcmp(k, "exec") || !strcmp(k, "execstart")) cfg_set(c->exec, sizeof c->exec, v);
        else if (!strcmp(k, "user")) cfg_set(c->user, sizeof c->user, v);
        else if (!strcmp(k, "needs") || !strcmp(k, "requires")) cfg_set(c->needs, sizeof c->needs, v);
        else if (!strcmp(k, "after")) cfg_set(c->after, sizeof c->after, v);
        else if (!strcmp(k, "dir") || !strcmp(k, "workingdirectory")) cfg_set(c->dir, sizeof c->dir, v);
        else if (!strcmp(k, "ready")) cfg_set(c->ready, sizeof c->ready, v);
        else if (!strcmp(k, "env") || !strcmp(k, "environment")) {
            size_t l = strlen(c->env);
            if (l + strlen(v) + 2 < sizeof c->env) snprintf(c->env + l, sizeof c->env - l, "%s%s", l ? "\n" : "", v);
        } else if (!strcmp(k, "type")) {
            if (!strcmp(v, "simple")) c->type = T_SIMPLE;
            else if (!strcmp(v, "oneshot")) c->type = T_ONESHOT;
            else ulog(LOG_WARNING, "%s:%d: type is simple or oneshot, not %s", where, lineno, v);
        } else if (!strcmp(k, "restart")) {
            if (!strcmp(v, "no")) c->restart = R_NO;
            else if (!strcmp(v, "on-failure")) c->restart = R_ON_FAILURE;
            else if (!strcmp(v, "always")) c->restart = R_ALWAYS;
            else ulog(LOG_WARNING, "%s:%d: restart is no, on-failure or always, not %s", where, lineno, v);
        } else if (!strcmp(k, "restart_delay") || !strcmp(k, "restartsec")) c->restart_ms = parse_ms(v);
        else if (!strcmp(k, "start_limit")) c->start_limit = atoi(v);
        else if (!strcmp(k, "target") || !strcmp(k, "wantedby")) {
            c->targets = 0;
            if (strstr(v, "multi-user")) c->targets |= TGT_MULTI;
            if (strstr(v, "rescue")) c->targets |= TGT_RESCUE;
            if (strstr(v, "install")) c->targets |= TGT_INSTALL;
            if (strstr(v, "graphical")) c->targets |= TGT_GRAPHICAL;
        } else if (!strcmp(k, "console")) c->console = yes;
        else if (!strcmp(k, "system_job")) c->system_job = yes;
        else if (!strcmp(k, "keep_processes")) c->keep_processes = yes;
        else if (!strcmp(k, "log")) c->log_console = !strcmp(v, "console");
        else if (!strcmp(k, "enabled")) c->enabled_default = yes;
        else ulog(LOG_WARNING, "%s:%d: unknown key %s", where, lineno, k);
    }
    if (c->console) c->log_console = 1;
    if (!c->exec[0]) ulog(LOG_WARNING, "%s: no exec= line", where);
}

static unit_t *add_unit(const char *name) {
    unit_t *u = find_unit(name);
    if (u || nunits == MAX_UNITS) return u;
    u = &units[nunits++];
    memset(u, 0, sizeof *u);
    cfg_set(u->c.name, sizeof u->c.name, name);
    u->last_code = 0;
    return u;
}

static int valid_unit_name(const char *s) {
    if (!*s || strlen(s) >= 28) return 0;
    for (; *s; s++)
        if (!((*s >= 'a' && *s <= 'z') || (*s >= 'A' && *s <= 'Z') || (*s >= '0' && *s <= '9') || *s == '-' || *s == '_' || *s == '.' || *s == '@'))
            return 0;
    return 1;
}

static int unit_active(const unit_t *u) {
    return u->state == U_WAITING || u->state == U_STARTING || u->state == U_RUNNING || u->state == U_STOPPING || u->state == U_RESTART_WAIT;
}

// Read /etc/svc (again): new units are added, changed ones take the new
// settings the next time they start, removed ones go once they are stopped
static void svc_load(void) {
    for (int i = 0; i < nunits; i++) units[i].present = 0;
    DIR *etc = opendir(SVC_DIR);
    if (etc) closedir(etc);
    svc_from_files = etc != 0;
    if (svc_from_files) {
        // The image's units, then /etc/svc's (the same name replaces)
        for (int vendor = 1; vendor >= 0; vendor--) {
            const char *dir = vendor ? VENDOR_DIR : SVC_DIR;
            DIR *d = opendir(dir);
            if (!d) continue;
            struct dirent *e;
            while ((e = readdir(d))) {
                size_t l = strlen(e->d_name);
                if (l < 5 || strcmp(e->d_name + l - 4, ".svc")) continue;
                char name[32];
                snprintf(name, sizeof name, "%.*s", (int)(l - 4), e->d_name);
                if (!valid_unit_name(name) || !strcmp(name, "emergency") || !strcmp(name, "boot-complete")) {
                    ulog(LOG_WARNING, "%s/%s: not a usable unit name", dir, e->d_name);
                    continue;
                }
                char path[300], text[8192];
                snprintf(path, sizeof path, "%s/%s", dir, e->d_name);
                int fd = open(path, O_RDONLY);
                if (fd < 0) continue;
                ssize_t n = read(fd, text, sizeof text - 1);
                close(fd);
                text[n > 0 ? n : 0] = 0;
                unit_t *u = add_unit(name);
                if (!u) break;
                unit_parse(&u->c, name, text, path);
                u->present = 1;
                u->builtin = 0;
                u->vendor = vendor;
                struct stat st;
                snprintf(path, sizeof path, "%s/enabled/%s", SVC_DIR, name);
                u->enabled = stat(path, &st) == 0;
                if (!u->enabled && u->c.enabled_default) {
                    snprintf(path, sizeof path, "%s/disabled/%s", SVC_DIR, name);
                    u->enabled = stat(path, &st) != 0;
                }
            }
            closedir(d);
        }
    } else {
        for (size_t i = 0; i < sizeof builtin_units / sizeof builtin_units[0]; i++) {
            unit_t *u = add_unit(builtin_units[i][0]);
            if (!u) break;
            unit_parse(&u->c, builtin_units[i][0], builtin_units[i][1], "(built in)");
            u->present = u->builtin = u->enabled = 1;
        }
    }
    unit_t *bc = add_unit("boot-complete");
    if (bc) {
        unit_parse(&bc->c, "boot-complete", boot_complete_unit, "(built in)");
        bc->present = bc->builtin = 1;
        bc->enabled = 0;
    }
    unit_t *em = add_unit("emergency");
    if (em) {
        unit_parse(&em->c, "emergency", emergency_unit, "(built in)");
        em->present = em->builtin = 1;
        em->enabled = 0;
    }
    // Drop what is gone and not running
    for (int i = nunits - 1; i >= 0; i--)
        if (!units[i].present && !unit_active(&units[i])) units[i] = units[--nunits];
}

static void unit_fail(unit_t *u, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
static void unit_fail(unit_t *u, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(u->reason, sizeof u->reason, fmt, ap);
    va_end(ap);
    u->state = U_FAILED;
    u->since_ms = ms_now();
    if (u->job) {
        rx_task_kill(u->job);
        rx_handle_close(u->job);
        u->job = 0;
    }
    ulog(LOG_ERR, "%s failed: %s", u->c.name, u->reason);
    // On the screen during the boot only (until a session holds the console)
    if (!svc_shutting_down && !sh_ctl) printf("init: %s failed: %s\n", u->c.name, u->reason);
}

static int split_args(char *s, char **av, int max) {
    int n = 0;
    while (*s && n < max) {
        while (*s == ' ' || *s == '\t') s++;
        if (!*s) break;
        char *out = s, q = 0;
        av[n++] = out;
        while (*s && (q || (*s != ' ' && *s != '\t'))) {
            if (q && *s == q) q = 0;
            else if (!q && (*s == '"' || *s == '\'')) q = *s;
            else if (*s == '\\' && q != '\'' && s[1]) *out++ = *++s;
            else *out++ = *s;
            s++;
        }
        if (*s) s++;
        *out = 0;
    }
    av[n] = 0;
    return n;
}

// Hand the other end of a service's output socket to logd
static void log_stream(unit_t *u, rx_handle_t end, uint32_t uid) {
    rx_log_msg_t m;
    memset(&m, 0, sizeof m);
    m.op = RX_LOG_OP_STREAM;
    m.priority = LOG_INFO;
    m.pid = u->pid;
    m.uid = uid;
    snprintf(m.tag, sizeof m.tag, "%s", u->c.name);
    for (int tries = 0; tries < 2; tries++) {
        if (!log_connect()) break;
        rx_handle_t h = end;
        if (rx_channel_write(log_ch, 0, &m, sizeof m, &h, 1) == RX_OK) return;
        rx_handle_close(log_ch); // logd has restarted: a new connection
        log_ch = 0;
    }
    rx_handle_close(end);
}

static void unit_spawn(unit_t *u) {
    char exec[256], path[300];
    char *av[40];
    snprintf(exec, sizeof exec, "%s", u->c.exec);
    int ac = split_args(exec, av, 36);
    if (!ac) {
        unit_fail(u, "no exec= line");
        return;
    }
    if (av[0][0] == '/') snprintf(path, sizeof path, "%s", av[0]);
    else snprintf(path, sizeof path, "/bin/%s", av[0]);
    char *base = strrchr(path, '/') + 1;
    av[0] = base;
    // The console's login logs in the autologin user without a password
    if (u->c.console && !strcmp(path, "/bin/login") && ac == 1 && autologin[0]) {
        av[ac++] = "-f";
        av[ac++] = autologin;
        av[ac] = 0;
    }
    const char *uname = u->c.user[0] ? u->c.user : "root";
    struct passwd *pw = getpwnam(uname);
    struct passwd rootpw = { "root", "x", 0, 0, "root", "/root", "/bin/shell" };
    if (!pw && !strcmp(uname, "root")) pw = &rootpw; // no /etc/passwd: root anyway
    if (!pw) {
        unit_fail(u, "no user %s", uname);
        return;
    }
    rx_creds_t cr;
    udb_creds(pw, &cr);
    // The environment: the user's, PWD, TERM on the console, then env= lines
    static char envbuf[1400];
    char *envp[32];
    int ne = 0;
    size_t used = 0;
#define ENV(...) do { if (ne < 30 && used < sizeof envbuf - 1) { envp[ne++] = envbuf + used; \
        used += (size_t)snprintf(envbuf + used, sizeof envbuf - used, __VA_ARGS__) + 1; if (used > sizeof envbuf) used = sizeof envbuf; } } while (0)
    ENV("PATH=/bin");
    ENV("HOME=%s", pw->pw_dir);
    ENV("USER=%s", pw->pw_name);
    ENV("LOGNAME=%s", pw->pw_name);
    ENV("SHELL=%s", pw->pw_shell && *pw->pw_shell ? pw->pw_shell : "/bin/shell");
    ENV("PWD=%s", u->c.dir[0] ? u->c.dir : (u->c.console && cr.euid == 0 ? "/root" : "/"));
    if (u->c.console) ENV("TERM=linux");
    ENV("SERVICE=%s", u->c.name);
    for (const char *p = u->c.env; *p;) {
        size_t n = strcspn(p, "\n");
        ENV("%.*s", (int)n, p);
        p += n + (p[n] == '\n');
    }
    envp[ne] = 0;
#undef ENV
    if (u->job) rx_handle_close(u->job);
    u->job = 0;
    if (rx_job_create(rx_job_default, 0, &u->job) != RX_OK) u->job = 0;
    rx_spawn_handle_t hs[8];
    uint32_t nh = 0;
    rx_handle_t stream_end = 0;
    if (u->c.console) {
        rx_handle_t ours, theirs;
        rx_channel_create(0, &ours, &theirs);
        hs[nh++] = (rx_spawn_handle_t){ RX_PA_HND(RX_PA_FD, 0), console_copy() };
        hs[nh++] = (rx_spawn_handle_t){ RX_PA_HND(RX_PA_FD, 1), console_copy() };
        hs[nh++] = (rx_spawn_handle_t){ RX_PA_HND(RX_PA_FD, 2), console_copy() };
        hs[nh++] = (rx_spawn_handle_t){ RX_PA_USER0, theirs };
        if (sh_ctl) rx_handle_close(sh_ctl);
        sh_ctl = ours;
        // A fresh session starts with no leftover interrupt
        rx_object_signal(con_client, RX_CONSOLE_SIGNAL_INTERRUPT, 0);
    } else {
        rx_handle_t a = 0, a2 = 0;
        if (!u->c.log_console && log_connect() && rx_socket_create(0, &a, &stream_end) == RX_OK &&
            rx_handle_duplicate(a, RX_RIGHT_SAME_RIGHTS, &a2) == RX_OK) {
            hs[nh++] = (rx_spawn_handle_t){ RX_FD_INFO(1, RX_FD_PIPE), a };
            hs[nh++] = (rx_spawn_handle_t){ RX_FD_INFO(2, RX_FD_PIPE), a2 };
        } else {
            if (a) rx_handle_close(a);
            if (stream_end) rx_handle_close(stream_end);
            stream_end = 0;
            hs[nh++] = (rx_spawn_handle_t){ RX_PA_HND(RX_PA_FD, 1), console_copy() };
            hs[nh++] = (rx_spawn_handle_t){ RX_PA_HND(RX_PA_FD, 2), console_copy() };
        }
    }
    if (u->c.system_job || u->c.console) {
        rx_handle_t sj = 0;
        if (rx_handle_duplicate(rx_job_default, RX_RIGHT_SAME_RIGHTS, &sj) == RX_OK)
            hs[nh++] = (rx_spawn_handle_t){ RX_PA_JOB_SYSTEM, sj };
    }
    rx_status_t r = rx_spawn_as(u->job ? u->job : rx_job_default, &cr, path, av, envp, RX_SPAWN_CLONE_NS, hs, nh, &u->proc);
    if (r != RX_OK) {
        u->proc = 0;
        if (stream_end) rx_handle_close(stream_end);
        if (u->c.console && sh_ctl) {
            rx_handle_close(sh_ctl);
            sh_ctl = 0;
        }
        unit_fail(u, "cannot start %s: %s", path, strerror(errno));
        return;
    }
    rx_info_handle_basic_t bi;
    memset(&bi, 0, sizeof bi);
    rx_object_get_info(u->proc, RX_INFO_HANDLE_BASIC, &bi, sizeof bi, 0, 0);
    u->pid = bi.koid;
    if (stream_end) log_stream(u, stream_end, cr.euid);
    u->starts++;
    u->order = ++start_counter;
    u->recent[u->nrecent++ % 8] = mono_ns();
    u->since_ms = ms_now();
    u->reason[0] = 0;
    if (u->c.ready[0]) {
        u->state = U_STARTING;
        u->deadline = mono_ns() + READY_TIMEOUT_NS;
    } else {
        u->state = U_RUNNING;
    }
    ulog(LOG_INFO, "started %s (pid %llu, user %s)", u->c.name, (unsigned long long)u->pid, pw->pw_name);
}

// Up for those that wait on it: running (and ready), or a oneshot that has finished
static int dep_ready(const unit_t *d) {
    return (d->state == U_RUNNING && d->c.type == T_SIMPLE) || d->state == U_EXITED;
}

static void ensure_console(void);
static const char *target_name(int t);

static void svc_schedule(void) {
    if (svc_shutting_down) return;
    for (int changed = 1, rounds = 0; changed && rounds < MAX_UNITS * 2; rounds++) {
        changed = 0;
        for (int i = 0; i < nunits; i++) {
            unit_t *u = &units[i];
            if (u->state != U_WAITING) continue;
            int ok = 1, failed = 0;
            char list[200], *save = 0;
            snprintf(list, sizeof list, "%s", u->c.needs);
            for (char *w = strtok_r(list, " ,\t", &save); w && !failed; w = strtok_r(0, " ,\t", &save)) {
                unit_t *d = find_unit(w);
                if (!d || d == u) {
                    unit_fail(u, "it needs %s, which does not exist", w);
                    failed = 1;
                } else if (d->state == U_FAILED) {
                    unit_fail(u, "it needs %s, which failed", d->c.name);
                    failed = 1;
                } else {
                    if (d->state == U_INACTIVE) {
                        d->state = U_WAITING; // pulled in
                        changed = 1;
                    }
                    if (!dep_ready(d)) ok = 0;
                }
            }
            if (failed) {
                changed = 1;
                continue;
            }
            snprintf(list, sizeof list, "%s", u->c.after);
            for (char *w = strtok_r(list, " ,\t", &save); w; w = strtok_r(0, " ,\t", &save)) {
                unit_t *d = find_unit(w);
                // Wait while it is on its way up
                if (d && d != u && (d->state == U_WAITING || d->state == U_STARTING || (d->state == U_RUNNING && d->c.type == T_ONESHOT)))
                    ok = 0;
            }
            if (ok) {
                unit_spawn(u);
                changed = 1;
            }
        }
    }
    // Everything of a multi-user or graphical boot started: boot-complete
    if (!boot_complete_done && (svc_target == TGT_MULTI || svc_target == TGT_GRAPHICAL)) {
        int busy = 0;
        for (int i = 0; i < nunits; i++) {
            const unit_t *u = &units[i];
            if (u->state == U_WAITING || u->state == U_STARTING || (u->state == U_RUNNING && u->c.type == T_ONESHOT)) busy = 1;
        }
        unit_t *bc = find_unit("boot-complete");
        if (!busy && bc) {
            boot_complete_done = 1;
            ulog(LOG_INFO, "the %s target is up", target_name(svc_target));
            unit_spawn(bc);
        }
    }
    ensure_console();
}

// Nothing may leave the console empty: then the emergency root shell
static void ensure_console(void) {
    if (svc_shutting_down) return;
    unit_t *em = 0;
    for (int i = 0; i < nunits; i++) {
        unit_t *u = &units[i];
        if (!strcmp(u->c.name, "emergency")) em = u;
        if (u->c.console && unit_active(u)) return;
    }
    if (!em) return;
    printf("\ninit: nothing runs on the console (`svc list` shows why); starting a root shell.\n"
           "init: `svc start console` brings the login back.\n\n");
    ulog(LOG_WARNING, "nothing runs on the console; starting the emergency shell");
    unit_spawn(em);
}

static void unit_start(unit_t *u) {
    if (unit_active(u) && u->state != U_STOPPING && u->state != U_RESTART_WAIT) return;
    if (u->state == U_STOPPING) {
        u->restart_pending = 1;
        return;
    }
    u->state = U_WAITING;
    u->reason[0] = 0;
}

static void unit_stop(unit_t *u) {
    u->restart_pending = 0;
    if (u->proc) {
        if (u->state != U_STOPPING) ulog(LOG_INFO, "stopping %s", u->c.name);
        u->state = U_STOPPING;
        rx_task_kill(u->job ? u->job : u->proc);
    } else if (u->state != U_FAILED) {
        u->state = U_INACTIVE;
        u->since_ms = ms_now();
    }
}

static int recent_starts(unit_t *u) {
    int n = 0;
    int64_t now = mono_ns();
    for (int k = 0; k < 8 && k < u->nrecent; k++)
        if (now - u->recent[k] < START_WINDOW_NS) n++;
    return n;
}

// Its main process has ended (the rest of its job goes with it, unless
// keep_processes: the mounts' filesystem servers stay, and end when the
// filesystems are shut down)
static void unit_ended(unit_t *u) {
    int64_t code = rx_wait_process(u->proc);
    u->proc = 0;
    if (u->job && u->c.keep_processes) {
        if (u->kept_job) rx_handle_close(u->kept_job);
        u->kept_job = u->job;
        u->job = 0;
    } else if (u->job) {
        rx_task_kill(u->job);
        rx_handle_close(u->job);
        u->job = 0;
    }
    u->last_code = code;
    u->have_code = 1;
    u->pid = 0;
    u->since_ms = ms_now();
    if (u->c.console) {
        console_output(); // its last words
        if (sh_ctl) rx_handle_close(sh_ctl);
        sh_ctl = 0;
    }
    char how[48];
    if (code == RX_TASK_RETCODE_SYSCALL_KILL) snprintf(how, sizeof how, "was killed");
    else if (code == RX_TASK_RETCODE_EXCEPTION_KILL) snprintf(how, sizeof how, "crashed (a CPU exception)");
    else snprintf(how, sizeof how, "exited with code %lld", (long long)code);
    if (u->state == U_STOPPING || svc_shutting_down) {
        u->state = U_INACTIVE;
        snprintf(u->reason, sizeof u->reason, "stopped");
        ulog(LOG_INFO, "%s stopped", u->c.name);
        if (u->restart_pending && !svc_shutting_down) {
            u->restart_pending = 0;
            u->state = U_WAITING;
        }
        return;
    }
    if (u->c.type == T_ONESHOT) {
        if (code == 0) {
            u->state = U_EXITED;
            u->reason[0] = 0;
            ulog(LOG_INFO, "%s finished", u->c.name);
        } else {
            unit_fail(u, "it %s", how);
        }
        return;
    }
    int again = u->c.restart == R_ALWAYS || (u->c.restart == R_ON_FAILURE && code != 0);
    if (!again) {
        if (code != 0) {
            unit_fail(u, "it %s", how);
        } else {
            u->state = U_INACTIVE;
            snprintf(u->reason, sizeof u->reason, "it exited");
            ulog(LOG_INFO, "%s exited", u->c.name);
        }
        return;
    }
    if (u->c.start_limit > 0 && recent_starts(u) >= u->c.start_limit) {
        unit_fail(u, "it %s, after %d starts in 30 s (svc reset-failed %s, then svc start)", how, recent_starts(u), u->c.name);
        return;
    }
    if (u->c.console) printf("\nSession ended; starting %s again\n\n", u->c.name);
    ulog(code ? LOG_WARNING : LOG_INFO, "%s %s; restarting it in %d.%03d s", u->c.name, how, u->c.restart_ms / 1000, u->c.restart_ms % 1000);
    snprintf(u->reason, sizeof u->reason, "it %s", how);
    u->state = U_RESTART_WAIT;
    u->deadline = mono_ns() + (int64_t)u->c.restart_ms * 1000000;
}

// Timers: restarts that are due, services that have become ready
static void svc_tick(void) {
    int64_t now = mono_ns();
    for (int i = 0; i < nunits; i++) {
        unit_t *u = &units[i];
        if (u->state == U_RESTART_WAIT && now >= u->deadline) u->state = U_WAITING;
        if (u->state == U_STARTING) {
            rx_handle_t h;
            if (rx_service_connect(u->c.ready, &h) == 0) {
                rx_handle_close(h);
                u->state = U_RUNNING;
            } else if (now >= u->deadline) {
                ulog(LOG_WARNING, "%s: %s did not appear in 10 s; going on", u->c.name, u->c.ready);
                u->state = U_RUNNING;
            }
        }
    }
    svc_schedule();
}

static uint64_t svc_deadline(void) {
    int64_t d = INT64_MAX;
    for (int i = 0; i < nunits; i++) {
        if (units[i].state == U_RESTART_WAIT && units[i].deadline < d) d = units[i].deadline;
        if (units[i].state == U_STARTING) {
            int64_t poll = mono_ns() + 50000000; // look again in 50 ms
            if (poll < d) d = poll;
        }
    }
    return d == INT64_MAX ? RX_TIME_INFINITE : (uint64_t)(d < 0 ? 0 : d);
}

static const char *target_name(int t) {
    return t == TGT_RESCUE ? "rescue" : t == TGT_INSTALL ? "install" : t == TGT_GRAPHICAL ? "graphical" : "multi-user";
}

static int target_of_cmdline(int argc, char **argv) {
    int t = TGT_MULTI;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "rescue") || !strcmp(argv[i], "single") || !strcmp(argv[i], "target=rescue") || !strcmp(argv[i], "S"))
            t = TGT_RESCUE;
        else if (!strcmp(argv[i], "target=multi-user")) t = TGT_MULTI;
        else if (!strcmp(argv[i], "installer") || !strcmp(argv[i], "target=install")) t = TGT_INSTALL;
        else if (!strcmp(argv[i], "desktop") || !strcmp(argv[i], "target=graphical")) t = TGT_GRAPHICAL;
    }
    return t;
}

static void svc_boot(int argc, char **argv) {
    svc_target = target_of_cmdline(argc, argv);
    svc_load();
    if (!svc_from_files) printf("init: no %s: using the built-in services\n", SVC_DIR);
    if (svc_target == TGT_RESCUE)
        printf("init: rescue mode: the system log, the mounts and a root shell on the console\n"
               "init: (`svc isolate multi-user` starts the rest, or reboot)\n");
    for (int i = 0; i < nunits; i++)
        if (units[i].enabled && in_target(units[i].c.targets, svc_target)) units[i].state = U_WAITING;
    svc_schedule();
}

// Power-off: every unit ends, logd last so it can record the rest
static void svc_stop_all(void) {
    svc_shutting_down = 1;
    ulog(LOG_NOTICE, "stopping the services");
    for (int pass = 0; pass < 2; pass++) {
        for (int i = 0; i < nunits; i++) {
            unit_t *u = &units[i];
            int last = !strcmp(u->c.name, "logd");
            if (u->proc && last == pass) rx_task_kill(u->job ? u->job : u->proc);
        }
        int64_t until = mono_ns() + 2000000000;
        for (int i = 0; i < nunits; i++) {
            unit_t *u = &units[i];
            int last = !strcmp(u->c.name, "logd");
            if (!u->proc || last != pass) continue;
            uint32_t pend = 0;
            int64_t left = until - mono_ns();
            rx_object_wait_one(u->proc, RX_PROCESS_TERMINATED, rx_clock_get_monotonic() + (uint64_t)(left > 0 ? left : 0), &pend);
            if (pend & RX_PROCESS_TERMINATED) unit_ended(u);
        }
        if (pass == 0) ulog(LOG_NOTICE, "the services have stopped; writing out the disks");
        if (pass == 0) usleep(100000); // logd writes that down
    }
}

// ---- requests (svc, systemctl) ----

static char svc_reply[60000];
static size_t svc_rlen;

static void reply_add(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void reply_add(const char *fmt, ...) {
    if (svc_rlen >= sizeof svc_reply - 1) return;
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(svc_reply + svc_rlen, sizeof svc_reply - svc_rlen, fmt, ap);
    va_end(ap);
    if (n > 0) svc_rlen += (size_t)n;
    if (svc_rlen > sizeof svc_reply - 1) svc_rlen = sizeof svc_reply - 1;
}

static void unit_line(const unit_t *u) {
    char tg[48] = "";
    if (u->c.targets & TGT_MULTI) strcat(tg, "multi-user");
    if (u->c.targets & TGT_RESCUE) strcat(tg, tg[0] ? ",rescue" : "rescue");
    if (u->c.targets & TGT_INSTALL) strcat(tg, tg[0] ? ",install" : "install");
    if (u->c.targets & TGT_GRAPHICAL) strcat(tg, tg[0] ? ",graphical" : "graphical");
    const char *en = !strcmp(u->c.name, "emergency") || !strcmp(u->c.name, "boot-complete") ? "static" : u->builtin ? "builtin" : u->enabled ? "enabled" : "disabled";
    char code[24] = "-";
    if (u->have_code) snprintf(code, sizeof code, "%lld", (long long)u->last_code);
    reply_add("%s\t%s\t%s\t%llu\t%lld\t%d\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", u->c.name, ustate_names[u->state], en,
              (unsigned long long)u->pid, (long long)u->since_ms, u->starts, code, tg[0] ? tg : "-",
              u->c.user[0] ? u->c.user : "root", u->c.type == T_ONESHOT ? "oneshot" : "simple",
              u->c.restart == R_ALWAYS ? "always" : u->c.restart == R_ON_FAILURE ? "on-failure" : "no", u->c.exec,
              u->c.desc[0] ? u->c.desc : "-", u->reason[0] ? u->reason : "-");
}

static int set_enabled(unit_t *u, int on) {
    if (!svc_from_files || u->builtin) {
        reply_add("%s is built in (there is no %s); it cannot be enabled or disabled\n", u->c.name, SVC_DIR);
        return -EINVAL;
    }
    char path[128];
    if (u->c.enabled_default) {
        // The image enables it: disabling leaves a mark in /etc/svc/disabled
        mkdir(SVC_DIR "/disabled", 0755);
        snprintf(path, sizeof path, "%s/disabled/%s", SVC_DIR, u->c.name);
        if (on) {
            if (unlink(path) < 0 && errno != ENOENT) return -errno;
            if (!u->enabled) reply_add("Enabled %s (removed %s)\n", u->c.name, path);
        } else {
            int fd = open(path, O_WRONLY | O_CREAT, 0644);
            if (fd < 0) return -errno;
            close(fd);
            if (u->enabled) reply_add("Disabled %s (%s)\n", u->c.name, path);
        }
        u->enabled = on;
        ulog(LOG_INFO, "%s %s", u->c.name, on ? "enabled" : "disabled");
        return 0;
    }
    mkdir(SVC_DIR "/enabled", 0755);
    snprintf(path, sizeof path, "%s/enabled/%s", SVC_DIR, u->c.name);
    if (on) {
        int fd = open(path, O_WRONLY | O_CREAT, 0644);
        if (fd < 0) return -errno;
        close(fd);
        if (!u->enabled) reply_add("Enabled %s (%s)\n", u->c.name, path);
    } else {
        if (unlink(path) < 0 && errno != ENOENT) return -errno;
        if (u->enabled) reply_add("Disabled %s (removed %s)\n", u->c.name, path);
    }
    u->enabled = on;
    ulog(LOG_INFO, "%s %s", u->c.name, on ? "enabled" : "disabled");
    return 0;
}

static void svc_isolate(int t) {
    svc_target = t;
    ulog(LOG_NOTICE, "switching to the %s target", target_name(t));
    for (int i = 0; i < nunits; i++) {
        unit_t *u = &units[i];
        int in = u->enabled && in_target(u->c.targets, t);
        if (in) unit_start(u);
        else if (unit_active(u)) unit_stop(u);
    }
}

// One request; the reply goes in svc_reply (status first)
static void svc_request(const rx_svc_req_t *rq, uint32_t len, const rx_sender_t *who) {
    int32_t st = 0;
    svc_rlen = 4;
    char name[64];
    snprintf(name, sizeof name, "%s", len >= sizeof *rq ? rq->name : "");
    uint32_t cmd = len >= 8 ? rq->cmd : 0;
    unit_t *u = name[0] ? find_unit(name) : 0;
    if (cmd != RX_SVC_LIST && cmd != RX_SVC_STATUS && who->euid != 0) {
        st = -EPERM;
    } else if (cmd == RX_SVC_LIST) {
        reply_add("target\t%s\n", target_name(svc_target));
        for (int i = 0; i < nunits; i++) unit_line(&units[i]);
    } else if (cmd == RX_SVC_RELOAD) {
        svc_load();
        reply_add("Read %s: %d units\n", svc_from_files ? SVC_DIR : "the built-in units", nunits);
        ulog(LOG_INFO, "read %s again (%d units)", SVC_DIR, nunits);
    } else if (cmd == RX_SVC_ISOLATE) {
        if (!strcmp(name, "rescue") || !strcmp(name, "rescue.target")) svc_isolate(TGT_RESCUE);
        else if (!strcmp(name, "install")) svc_isolate(TGT_INSTALL);
        else if (!strcmp(name, "multi-user") || !strcmp(name, "multi-user.target") || !strcmp(name, "default")) svc_isolate(TGT_MULTI);
        else if (!strcmp(name, "graphical") || !strcmp(name, "graphical.target")) svc_isolate(TGT_GRAPHICAL);
        else {
            reply_add("No target %s (rescue, multi-user, graphical, install)\n", name);
            st = -ENOENT;
        }
    } else if (cmd == RX_SVC_RESET_FAILED && !name[0]) {
        for (int i = 0; i < nunits; i++)
            if (units[i].state == U_FAILED) units[i].state = U_INACTIVE, units[i].nrecent = 0;
    } else if (!u) {
        reply_add("No unit %s (svc list shows them; files are %s/NAME.svc)\n", name, SVC_DIR);
        st = -ENOENT;
    } else if (cmd == RX_SVC_STATUS) {
        unit_line(u);
    } else if (cmd == RX_SVC_START) {
        if (u->state == U_FAILED) u->nrecent = 0;
        unit_start(u);
    } else if (cmd == RX_SVC_STOP) {
        unit_stop(u);
    } else if (cmd == RX_SVC_RESTART) {
        if (u->state == U_FAILED) u->nrecent = 0;
        if (u->proc) {
            unit_stop(u);
            u->restart_pending = 1;
        } else {
            u->state = U_INACTIVE;
            unit_start(u);
        }
    } else if (cmd == RX_SVC_ENABLE || cmd == RX_SVC_DISABLE) {
        st = set_enabled(u, cmd == RX_SVC_ENABLE);
    } else if (cmd == RX_SVC_RESET_FAILED) {
        if (u->state == U_FAILED) u->state = U_INACTIVE;
        u->nrecent = 0;
    } else {
        st = -ENOSYS;
    }
    memcpy(svc_reply, &st, 4);
    svc_schedule();
}

#endif
