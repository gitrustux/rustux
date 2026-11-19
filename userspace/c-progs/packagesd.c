// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// packagesd: the installer service, /svc/packages (package step E6;
// FoxDen step FD4 adds apps to it). It runs rpg for the desktop and for
// users who are not root, so that nothing in the session needs sudo
// (rpg itself asks it when not run as root):
//
//   status                the last check (below); anyone
//   check                 rpg update, then rpg upgrade --plan; anyone
//   update                the same, rpg update's lines sent as it runs
//   install NAME|/FILE.rpg...     rpg install -y
//   remove [--purge] NAME...      rpg remove
//   upgrade               rpg upgrade -y
//
// For install, remove and upgrade it asks rpg first what would change
// (rpg plan) and decides: apps from app sources that ask for neither
// files:home nor system may be installed, upgraded and removed by anyone;
// anything else (a system package, a package file, such an app, --purge,
// which removes every user's data for an app) needs an administrator: the
// sender is root, or a second line of the request authenticates one:
//
//   PASSWORD              the sender's own, if they are in group sudo
//   as NAME PASSWORD      an administrator's (someone not one asks one)
//
// Without it the reply is "auth WHY" (send the request again with the
// second line), or "denied WHY". While rpg runs, "out LINE" messages, then
// "done CODE". The plan and the change happen under one lock, so nothing
// changes in between.
//
// A request is one channel message of text; so is each reply. One
// operation runs at a time. Every day (/etc/rpg/updates.conf: `check
// daily` or `never`; `delay SECONDS` before the first one, 120) it checks
// by itself; also soon after the sources or keys change, and every hour
// while the last check failed. What it found is in /var/lib/rpg/updates, which anyone may
// read (the panel and Settings > Updates do):
//
//   checked SECONDS       when (UTC seconds; 0: never)
//   error TEXT            why the check failed ("" when it did not)
//   step NAME FROM TO SIZE WHY    rpg upgrade --plan's lines
//   kept NAME REASON
//   upgraded SECONDS CODE the last upgrade, and how it ended

#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pthread.h>
#include <pwd.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <syslog.h>
#include <time.h>
#include <unistd.h>

#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/syscalls.h>

#include "userdb.h"

#define STATUS "/var/lib/rpg/updates"
#define RPG "/bin/rpg"

extern char **environ;

static pthread_mutex_t busy = PTHREAD_MUTEX_INITIALIZER;   // one rpg at a time
static pthread_mutex_t udb_lock = PTHREAD_MUTEX_INITIALIZER;
static long long last_upgrade;
static int last_upgrade_code = -1;

// Run argv (stdout and stderr together), each line to fn; the exit code
static int run(char *const argv[], void (*fn)(const char *line, void *arg), void *arg) {
    int p[2];
    if (pipe(p) < 0) return -1;
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_adddup2(&fa, p[1], 1);
    posix_spawn_file_actions_adddup2(&fa, p[1], 2);
    pid_t pid;
    int r = posix_spawn(&pid, argv[0], &fa, 0, argv, environ);
    posix_spawn_file_actions_destroy(&fa);
    close(p[1]);
    if (r != 0) {
        close(p[0]);
        return -1;
    }
    char buf[4096];
    size_t have = 0;
    for (;;) {
        ssize_t n = read(p[0], buf + have, sizeof buf - 1 - have);
        if (n <= 0) break;
        have += (size_t)n;
        buf[have] = 0;
        char *s = buf, *nl;
        while ((nl = strchr(s, '\n'))) {
            *nl = 0;
            if (fn) fn(s, arg);
            s = nl + 1;
        }
        have = strlen(s);
        memmove(buf, s, have + 1);
        if (have == sizeof buf - 1) {   // a very long line: as it is
            if (fn) fn(buf, arg);
            have = 0;
        }
    }
    if (have && fn) fn(buf, arg);
    close(p[0]);
    int st = 0;
    if (waitpid(pid, &st, 0) < 0) return -1;
    return WIFEXITED(st) ? WEXITSTATUS(st) : 128;
}

struct text {
    char *s;
    size_t len, cap;
};

static void text_add(struct text *t, const char *line) {
    size_t l = strlen(line);
    if (t->len + l + 2 > t->cap) {
        size_t cap = (t->cap + l + 2) * 2;
        char *n = realloc(t->s, cap);
        if (!n) return;
        t->s = n;
        t->cap = cap;
    }
    memcpy(t->s + t->len, line, l);
    t->len += l;
    t->s[t->len++] = '\n';
    t->s[t->len] = 0;
}

static void collect(const char *line, void *arg) { text_add(arg, line); }

// The status file's text, or a fresh one
static char *read_status(void) {
    FILE *f = fopen(STATUS, "r");
    struct text t = { 0, 0, 0 };
    char line[1024];
    if (f) {
        while (fgets(line, sizeof line, f)) {
            line[strcspn(line, "\n")] = 0;
            text_add(&t, line);
        }
        fclose(f);
    }
    if (!t.s) text_add(&t, "checked 0");
    return t.s;
}

static void write_status(long long checked, const char *error, const char *plan, const char *apps) {
    mkdir("/var/lib/rpg", 0755);
    char tmp[64];
    snprintf(tmp, sizeof tmp, "%s.new", STATUS);
    FILE *f = fopen(tmp, "w");
    if (!f) return;
    fprintf(f, "checked %lld\nerror %s\n%s%s", checked, error ? error : "", plan ? plan : "", apps ? apps : "");
    if (last_upgrade) fprintf(f, "upgraded %lld %d\n", last_upgrade, last_upgrade_code);
    fclose(f);
    chmod(tmp, 0644);
    rename(tmp, STATUS);
}

// A field of the current status ("checked"), or 0
static long long status_number(const char *key) {
    char *s = read_status();
    long long v = 0;
    size_t kl = strlen(key);
    for (char *l = s; l && *l;) {
        char *nl = strchr(l, '\n');
        if (!strncmp(l, key, kl) && l[kl] == ' ') v = atoll(l + kl + 1);
        l = nl ? nl + 1 : 0;
    }
    free(s);
    return v;
}

struct stream {
    rx_handle_t ch;
    const char *what;   // for the log ("upgrade", "install")
};

static void send_text(rx_handle_t ch, const char *s);

// A line of rpg update for check() to keep, and to send if asked
struct check_out {
    struct text *t;
    rx_handle_t ch;
};

static void check_line(const char *line, void *arg) {
    struct check_out *c = arg;
    text_add(c->t, line);
    if (c->ch) {
        char msg[4200];
        snprintf(msg, sizeof msg, "out %s", line);
        send_text(c->ch, msg);
    }
}

// /etc/rpg/updates.conf's value for `key` (into v), or def
static void conf_get(const char *key, char *v, size_t cap, const char *def) {
    snprintf(v, cap, "%s", def);
    FILE *f = fopen("/etc/rpg/updates.conf", "r");
    if (!f) return;
    char line[256];
    while (fgets(line, sizeof line, f)) {
        if (line[0] == '#') continue;
        char *k = strtok(line, " \t\r\n"), *val = strtok(0, " \t\r\n");
        if (k && val && !strcmp(k, key)) snprintf(v, cap, "%s", val);
    }
    fclose(f);
}

// The permissions an installed app asks for (its manifest), ","-joined
// with "," at both ends (",network,files:documents,"; ",," for none)
static void installed_perms(const char *id, char *out, size_t cap) {
    char p[300], line[512];
    snprintf(out, cap, ",");
    snprintf(p, sizeof p, "/apps/%s/manifest", id);
    FILE *f = fopen(p, "r");
    if (!f) return;
    while (fgets(line, sizeof line, f)) {
        if (strncmp(line, "Permissions:", 12)) continue;
        for (char *w = strtok(line + 12, " \t\r\n"); w; w = strtok(0, " \t\r\n")) {
            size_t l = strlen(out);
            snprintf(out + l, cap - l, "%s,", w);
        }
    }
    fclose(f);
    if (!strcmp(out, ",")) snprintf(out, cap, ",,");
}

// The status's lines starting with `key`, the newest `keep` (history)
static void keep_lines(const char *key, int keep, struct text *t) {
    char *s = read_status();
    char *lines[64];
    int n = 0;
    size_t kl = strlen(key);
    for (char *l = s; l && *l;) {
        char *nl = strchr(l, '\n');
        if (nl) *nl = 0;
        if (!strncmp(l, key, kl) && l[kl] == ' ') {
            if (n == 64) {
                memmove(lines, lines + 1, sizeof lines[0] * 63);
                n--;
            }
            lines[n++] = l;
        }
        l = nl ? nl + 1 : 0;
    }
    for (int i = n > keep ? n - keep : 0; i < n; i++) text_add(t, lines[i]);
    free(s);
}

// A line of rpg's for the log only
static void log_line(const char *line, void *arg) {
    syslog(LOG_INFO, "%s: %s", (const char *)arg, line);
}

// Text a struct text holds, its lines added to another (no blank line)
static void text_append(struct text *to, struct text *from) {
    for (char *l = from->s; l && *l;) {
        char *nl = strchr(l, '\n');
        if (nl) *nl = 0;
        if (*l) text_add(to, l);
        l = nl ? nl + 1 : 0;
    }
}

// App updates (FoxDen step FD6): after the check, an app whose new version
// asks for no more than the installed one is upgraded at once (rpg install
// -y ID); one that asks for more waits for the user ("appwaits ID FROM TO
// PERMISSIONS"). /etc/rpg/updates.conf: apps auto (the default) | ask
// (none at once). Lines for the status (appwaits, and appupdated SECONDS
// ID FROM TO: the newest 20) added to `out`. Under `busy`.
static void app_updates(struct text *out) {
    char mode[16];
    conf_get("apps", mode, sizeof mode, "auto");
    struct text plan = { 0, 0, 0 }, done = { 0, 0, 0 };
    char *pv[] = { RPG, "plan", "upgrade", 0 };
    keep_lines("appupdated", 20, &done);
    if (run(pv, collect, &plan) != 0) {
        free(plan.s);
        text_append(out, &done);
        free(done.s);
        return;
    }
    char *argv[44] = { RPG, "install", "-y" };
    char froms[40][64], tos[40][64];
    int n = 3, k = 0;
    for (char *l = plan.s; l && *l;) {
        char *nl = strchr(l, '\n');
        if (nl) *nl = 0;
        // step upgrade NAME FROM TO app apps PERMISSIONS
        char line[600], *f[8], *save = 0;
        int nf = 0;
        snprintf(line, sizeof line, "%s", l);
        for (char *t = strtok_r(line, " ", &save); t && nf < 8; t = strtok_r(0, " ", &save)) f[nf++] = t;
        l = nl ? nl + 1 : 0;
        if (nf != 8 || strcmp(f[0], "step") || strcmp(f[1], "upgrade") || strcmp(f[5], "app") || strcmp(f[6], "apps")) continue;
        char old[300], np[300];
        installed_perms(f[2], old, sizeof old);
        int more = 0;
        if (strcmp(f[7], "-")) {
            snprintf(np, sizeof np, "%s", f[7]);
            for (char *w = strtok_r(np, ",", &save); w; w = strtok_r(0, ",", &save)) {
                char key[80];
                snprintf(key, sizeof key, ",%s,", w);
                if (!strstr(old, key)) more = 1;
            }
        }
        if (more || strcmp(mode, "auto")) {
            if (more) {
                char w[700];
                snprintf(w, sizeof w, "appwaits %s %s %s %s", f[2], f[3], f[4], f[7]);
                text_add(out, w);
                syslog(LOG_INFO, "app update waits for the user: %s %s asks for more (%s)", f[2], f[4], f[7]);
            }
            continue;
        }
        if (n < 43 && k < 40) {
            argv[n++] = strdup(f[2]);
            snprintf(froms[k], sizeof froms[k], "%s", f[3]);
            snprintf(tos[k], sizeof tos[k], "%s", f[4]);
            k++;
        }
    }
    free(plan.s);
    if (k) {
        argv[n] = 0;
        int code = run(argv, log_line, "app update");
        for (int i = 0; i < k; i++) {
            if (code == 0) {
                char d[300];
                snprintf(d, sizeof d, "appupdated %lld %s %s %s", (long long)time(0), argv[3 + i], froms[i], tos[i]);
                text_add(&done, d);
                syslog(LOG_INFO, "app updated: %s %s -> %s", argv[3 + i], froms[i], tos[i]);
            }
            free(argv[3 + i]);
        }
        if (code) syslog(LOG_WARNING, "app updates failed (%d)", code);
    }
    text_append(out, &done);
    free(done.s);
}

// rpg update, then the plan: the new status. 0 if the check worked.
// ch: also send rpg update's lines there ("update")
static int check_to(rx_handle_t ch) {
    pthread_mutex_lock(&busy);
    struct text out = { 0, 0, 0 }, plan = { 0, 0, 0 };
    char *up[] = { RPG, "update", 0 };
    struct check_out co = { &out, ch };
    int r = run(up, check_line, &co);
    char err[300] = "";
    if (r != 0) {
        // the last line that says why
        const char *why = "rpg update failed";
        if (out.s) {
            char *last = 0;
            for (char *l = out.s; *l;) {
                char *nl = strchr(l, '\n');
                if (nl) *nl = 0;
                if (strstr(l, "REFUSED") || strstr(l, "rpg:") || strstr(l, "error")) last = l;
                if (!nl) break;
                l = nl + 1;
            }
            if (last) why = last;
        }
        snprintf(err, sizeof err, "%s", why);
    }
    // Apps first (some are upgraded now), then the plan of what is left;
    // the plan even after a failed source: the lists of the others are there
    struct text apps = { 0, 0, 0 };
    app_updates(&apps);
    char *pl[] = { RPG, "upgrade", "--plan", 0 };
    int rp = run(pl, collect, &plan);
    if (rp != 0 && !err[0]) snprintf(err, sizeof err, "%s", plan.s ? plan.s : "rpg upgrade --plan failed");
    err[strcspn(err, "\n")] = 0;
    write_status((long long)time(0), err, rp == 0 ? plan.s : "", apps.s);
    free(apps.s);
    int n = 0;
    for (char *l = plan.s; rp == 0 && l && *l; l = strchr(l, '\n') ? strchr(l, '\n') + 1 : 0)
        if (!strncmp(l, "step ", 5)) n++;
    if (err[0]) syslog(LOG_WARNING, "checked for updates: %s", err);
    else syslog(LOG_INFO, "checked for updates: %d package%s to upgrade", n, n == 1 ? "" : "s");
    free(out.s);
    free(plan.s);
    pthread_mutex_unlock(&busy);
    return err[0] ? -1 : 0;
}

static int check(void) { return check_to(0); }

static void send_text(rx_handle_t ch, const char *s) {
    size_t l = strlen(s);
    if (l > 60000) l = 60000;
    rx_channel_write(ch, 0, s, (uint32_t)l, 0, 0);
}

static void stream_line(const char *line, void *arg) {
    struct stream *st = arg;
    char msg[4200];
    snprintf(msg, sizeof msg, "out %s", line);
    send_text(st->ch, msg);
    syslog(LOG_INFO, "%s: %s", st->what, line);
}

static int in_sudo(const char *name, gid_t base) {
    struct group *g = getgrnam("sudo");
    if (!g) return 0;
    gid_t list[64];
    int n = 64;
    if (getgrouplist(name, base, list, &n) < 0) n = 64;
    for (int i = 0; i < n; i++)
        if (list[i] == g->gr_gid) return 1;
    return 0;
}

// Is an administrator's say given? The sender is root, or `auth` (the
// request's second line, or 0) authenticates one: the sender's own
// password when they are in group sudo, "as NAME PASSWORD" anyone's who
// is. 1 yes; 0 no (why: "" when nothing was given, so ask for it)
static int admin_ok(const rx_sender_t *who, char *auth, char *why, size_t cap) {
    why[0] = 0;
    if (who->euid == 0) return 1;
    if (!auth || !*auth) return 0;
    // getpwuid and getpwnam share one buffer: the sender's name copied
    struct passwd *p = getpwuid(who->uid);
    char me[64] = "";
    if (p) snprintf(me, sizeof me, "%s", p->pw_name);
    char name[64];
    const char *pw;
    if (!strncmp(auth, "as ", 3)) {
        char *n = auth + 3, *sp = strchr(n, ' ');
        if (!sp) {
            snprintf(why, cap, "as NAME PASSWORD");
            return 0;
        }
        snprintf(name, sizeof name, "%.*s", (int)(sp - n), n);
        pw = sp + 1;
    } else {
        if (!me[0]) {
            snprintf(why, cap, "unknown user %u", who->uid);
            return 0;
        }
        snprintf(name, sizeof name, "%s", me);
        pw = auth;
    }
    struct passwd *a = getpwnam(name);
    int sender_admin = me[0] && !strcmp(name, me);
    struct group *g = getgrnam("sudo");
    int admin = a && (sender_admin ? (g && rx_sender_in_group(who, g->gr_gid)) : in_sudo(name, a->pw_gid));
    if (!admin) {
        snprintf(why, cap, "%s is not an administrator (group sudo)", name);
        return 0;
    }
    pthread_mutex_lock(&udb_lock);
    int ok = udb_check(name, pw);
    pthread_mutex_unlock(&udb_lock);
    if (!ok) {
        sleep(2);
        snprintf(why, cap, "wrong password for %s", name);
        return 0;
    }
    snprintf(why, cap, "%s", name);   // who said yes, for the log
    return 1;
}

// The request's arguments as rpg's: names (lower-case letters, digits,
// + - .), absolute paths of package files (install), --purge (remove).
// Count, or -1 (why says what is wrong)
static int parse_args(const char *verb, char *line, char **argv, int max, int *files, char *why, size_t cap) {
    int n = 0;
    *files = 0;
    for (char *w = strtok(line, " "); w; w = strtok(0, " ")) {
        if (n == max) {
            snprintf(why, cap, "too many at once");
            return -1;
        }
        if (!strcmp(verb, "remove") && !strcmp(w, "--purge")) {
            argv[n++] = w;
            continue;
        }
        if (!strcmp(verb, "install") && w[0] == '/') {
            size_t l = strlen(w);
            if (l < 5 || strcmp(w + l - 4, ".rpg") || strstr(w, "/../")) {
                snprintf(why, cap, "%s: a package file is an absolute path ending in .rpg", w);
                return -1;
            }
            (*files)++;
            argv[n++] = w;
            continue;
        }
        for (char *c = w; *c; c++) {
            if (!((*c >= 'a' && *c <= 'z') || (*c >= '0' && *c <= '9') || *c == '+' || *c == '-' || *c == '.') || w[0] == '-') {
                snprintf(why, cap, "%s: not a package name", w);
                return -1;
            }
        }
        argv[n++] = w;
    }
    if (n == 0 || (n == 1 && !strcmp(argv[0], "--purge"))) {
        snprintf(why, cap, "%s what?", verb);
        return -1;
    }
    return n;
}

// From rpg plan's lines: why an administrator is needed ("" if not)
static void needs_admin(const char *plan, int purge, char *why, size_t cap) {
    why[0] = 0;
    int sys = 0;
    char first_sys[80] = "";
    for (const char *l = plan; l && *l;) {
        const char *nl = strchr(l, '\n');
        char line[512];
        snprintf(line, sizeof line, "%.*s", (int)(nl ? (size_t)(nl - l) : strlen(l)), l);
        l = nl ? nl + 1 : 0;
        // step OP NAME FROM TO KIND ORIGIN PERMISSIONS
        char *f[8], *save = 0;
        int nf = 0;
        for (char *t = strtok_r(line, " ", &save); t && nf < 8; t = strtok_r(0, " ", &save)) f[nf++] = t;
        if (nf != 8 || strcmp(f[0], "step")) continue;
        const char *name = f[2], *kind = f[5], *origin = f[6], *perms = f[7];
        if (strcmp(kind, "app")) {
            if (!sys++) snprintf(first_sys, sizeof first_sys, "%s", name);
            continue;
        }
        if (!strcmp(origin, "file") && !why[0]) snprintf(why, cap, "%s is a package file (not from a signed app source)", name);
        if (!strcmp(origin, "system") && !why[0]) snprintf(why, cap, "%s comes from a system source", name);
        char p[260];
        snprintf(p, sizeof p, ",%s,", perms);
        // What an app may do matters when it comes, not when it goes
        int removal = !strcmp(f[1], "remove");
        if (!removal && strstr(p, ",files:home,") && !why[0]) snprintf(why, cap, "%s asks for files:home (the whole home folder)", name);
        if (!removal && strstr(p, ",system,") && !why[0]) snprintf(why, cap, "%s asks for system (no sandbox)", name);
    }
    if (sys && !why[0]) {
        if (sys == 1) snprintf(why, cap, "%s is a system package", first_sys);
        else snprintf(why, cap, "%d system packages (%s, ...)", sys, first_sys);
    }
    if (purge && !why[0]) snprintf(why, cap, "--purge removes every user's data for it");
}

static void send_done(rx_handle_t ch, int code) {
    char msg[32];
    snprintf(msg, sizeof msg, "done %d", code);
    send_text(ch, msg);
}

static void deny(rx_handle_t ch, const char *verb, const char *why) {
    char msg[400];
    snprintf(msg, sizeof msg, "denied %s", why);
    send_text(ch, msg);
    syslog(LOG_WARNING, "%s refused: %s", verb, why);
}

// install, remove, upgrade: rpg plan, the decision, then rpg
static void change(rx_handle_t ch, const rx_sender_t *who, const char *verb, char *args, char *auth) {
    char why[300], who_ok[300];
    char *argv[48];
    int files = 0, n = 0;
    struct passwd *p = getpwuid(who->uid);
    char me[64];
    snprintf(me, sizeof me, "%s", p ? p->pw_name : "?");   // (getpwnam reuses the buffer)
    if (strcmp(verb, "upgrade")) {
        n = parse_args(verb, args ? args : (char *)"", argv, 40, &files, why, sizeof why);
        if (n < 0) {
            deny(ch, verb, why);
            return;
        }
    }
    int purge = 0;
    for (int i = 0; i < n; i++) purge |= !strcmp(argv[i], "--purge");
    int admin = admin_ok(who, auth, who_ok, sizeof who_ok);
    if (auth) memset(auth, 0, strlen(auth));
    if (!admin && who_ok[0]) {
        // A password that was wrong, or someone who is not an administrator
        char msg[400];
        snprintf(msg, sizeof msg, "%s %s", strstr(who_ok, "wrong password") ? "auth" : "denied", who_ok);
        send_text(ch, msg);
        syslog(LOG_WARNING, "%s asked by %s: %s", verb, me, who_ok);
        return;
    }
    // Package files only with an administrator's say, before rpg reads them
    if (files && !admin) {
        send_text(ch, "auth installing a package file");
        return;
    }
    pthread_mutex_lock(&busy);
    // What would change
    char *pv[52];
    int k = 0;
    pv[k++] = RPG;
    pv[k++] = "plan";
    pv[k++] = (char *)verb;
    for (int i = 0; i < n; i++) pv[k++] = argv[i];
    pv[k] = 0;
    struct text plan = { 0, 0, 0 };
    int r = run(pv, collect, &plan);
    if (r != 0) {
        // Why it cannot be done (no source has it, not installed...)
        for (char *l = plan.s; l && *l;) {
            char *nl = strchr(l, '\n');
            if (nl) *nl = 0;
            char msg[4200];
            snprintf(msg, sizeof msg, "out %s", l);
            send_text(ch, msg);
            l = nl ? nl + 1 : 0;
        }
        free(plan.s);
        pthread_mutex_unlock(&busy);
        send_done(ch, r);
        return;
    }
    needs_admin(plan.s, purge, why, sizeof why);
    free(plan.s);
    if (why[0] && !admin) {
        pthread_mutex_unlock(&busy);
        char msg[400];
        snprintf(msg, sizeof msg, "auth %s", why);
        send_text(ch, msg);
        syslog(LOG_INFO, "%s asked by %s: needs an administrator: %s", verb, me, why);
        return;
    }
    syslog(LOG_INFO, "%s asked by %s%s%s", verb, me, admin && who->euid ? ", allowed by " : "", admin && who->euid ? who_ok : "");
    // Now the change itself
    k = 0;
    pv[k++] = RPG;
    pv[k++] = (char *)verb;
    if (strcmp(verb, "remove")) pv[k++] = "-y";
    for (int i = 0; i < n; i++) pv[k++] = argv[i];
    pv[k] = 0;
    struct stream st = { ch, verb };
    int code = run(pv, stream_line, &st);
    if (!strcmp(verb, "upgrade")) {
        last_upgrade = (long long)time(0);
        last_upgrade_code = code;
    }
    pthread_mutex_unlock(&busy);
    check();   // what is left to upgrade
    send_done(ch, code);
}

struct client {
    rx_handle_t ch;
};

static void *serve(void *arg) {
    struct client *cl = arg;
    rx_handle_t ch = cl->ch;
    free(cl);
    char msg[4097];
    for (;;) {
        uint32_t nb = 0, nh = 0;
        rx_handle_t hs[1];
        rx_sender_t who;
        rx_status_t s = rx_channel_read_creds(ch, msg, hs, sizeof msg - 1, 1, &nb, &nh, &who);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t pend = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
            if (!(pend & RX_CHANNEL_READABLE)) break;
            continue;
        }
        if (s != RX_OK) break;
        for (uint32_t i = 0; i < nh; i++) rx_handle_close(hs[i]);
        msg[nb] = 0;
        char *nl = strchr(msg, '\n');
        char *rest = 0;
        if (nl) {
            *nl = 0;
            rest = nl + 1;
        }
        if (!strcmp(msg, "status")) {
            char *t = read_status();
            send_text(ch, t);
            free(t);
        } else if (!strcmp(msg, "check")) {
            check();
            char *t = read_status();
            send_text(ch, t);
            free(t);
        } else if (!strcmp(msg, "update")) {
            int r = check_to(ch);
            send_done(ch, r == 0 ? 0 : 1);
        } else if (!strcmp(msg, "upgrade")) {
            change(ch, &who, "upgrade", 0, rest);
        } else if (!strncmp(msg, "install ", 8) || !strncmp(msg, "remove ", 7)) {
            char *sp = strchr(msg, ' ');
            *sp = 0;
            change(ch, &who, msg, sp + 1, rest);
        } else {
            send_text(ch, "error status, check, update, install, remove or upgrade");
        }
        memset(msg, 0, sizeof msg);
    }
    rx_handle_close(ch);
    return 0;
}

// The newest change to the sources and keys (seconds)
static long long sources_changed(void) {
    const char *paths[] = { "/etc/rpg/sources.list", "/etc/rpg/sources.list.d", "/etc/rpg/keys" };
    long long t = 0;
    for (size_t i = 0; i < sizeof paths / sizeof paths[0]; i++) {
        struct stat st;
        if (stat(paths[i], &st) == 0 && (long long)st.st_mtime > t) t = (long long)st.st_mtime;
    }
    return t;
}

static int last_failed(void) {
    char *s = read_status();
    int failed = 0;
    for (char *l = s; l && *l;) {
        char *nl = strchr(l, '\n');
        if (!strncmp(l, "error ", 6) && l[6] && l[6] != '\n') failed = 1;
        l = nl ? nl + 1 : 0;
    }
    free(s);
    return failed;
}

// The daily check
static void *daily(void *arg) {
    (void)arg;
    char mode[32] = "daily";
    int delay = 120;
    FILE *f = fopen("/etc/rpg/updates.conf", "r");
    if (f) {
        char line[256];
        while (fgets(line, sizeof line, f)) {
            if (line[0] == '#') continue;
            char *k = strtok(line, " \t\r\n"), *v = strtok(0, " \t\r\n");
            if (!k || !v) continue;
            if (!strcmp(k, "check")) snprintf(mode, sizeof mode, "%s", v);
            else if (!strcmp(k, "delay")) delay = atoi(v);
        }
        fclose(f);
    }
    if (strcmp(mode, "daily")) {
        syslog(LOG_INFO, "no daily check (/etc/rpg/updates.conf: check %s)", mode);
        return 0;
    }
    sleep((unsigned)(delay > 0 ? delay : 0));
    for (;;) {
        long long last = status_number("checked"), now = (long long)time(0);
        if (now - last >= 24 * 3600 || last > now || sources_changed() >= last || (last_failed() && now - last >= 3600)) check();
        sleep(60);
    }
}

int main(void) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    if (geteuid() != 0) {
        fprintf(stderr, "packagesd: must run as root (init starts it)\n");
        return 1;
    }
    openlog("packagesd", 0, LOG_DAEMON);
    rx_handle_t mine, theirs;
    if (rx_channel_create(0, &mine, &theirs) != RX_OK || rx_service_publish("/svc/packages", theirs) < 0) {
        fprintf(stderr, "packagesd: cannot publish /svc/packages\n");
        return 1;
    }
    pthread_t th;
    if (pthread_create(&th, 0, daily, 0) == 0) pthread_detach(th);
    for (;;) {
        rx_svc_connect_t m;
        rx_handle_t h = 0;
        uint32_t nb = 0, nh = 0;
        rx_status_t s = rx_channel_read(mine, 0, &m, &h, sizeof m, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) {
            rx_object_wait_one(mine, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
            continue;
        }
        if (s != RX_OK) return 1;
        if (!nh) continue;
        struct client *cl = malloc(sizeof *cl);
        if (!cl) {
            rx_handle_close(h);
            continue;
        }
        cl->ch = h;
        if (pthread_create(&th, 0, serve, cl) != 0) {
            rx_handle_close(h);
            free(cl);
            continue;
        }
        pthread_detach(th);
    }
}
