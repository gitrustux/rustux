// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// packagesd: the installer service, /svc/packages (package step E6;
// FOXDEN.md step FD4 adds apps to it). It runs rpg for the desktop and for
// users who are not root, so that nothing in the session needs sudo:
//
//   status                the last check (below); anyone
//   check                 rpg update, then rpg upgrade --plan; anyone
//   upgrade\nPASSWORD     rpg upgrade -y: root, or an administrator (group
//                         sudo) with their own password. Messages
//                         "out LINE" while it runs, then "done CODE"
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

static void write_status(long long checked, const char *error, const char *plan) {
    mkdir("/var/lib/rpg", 0755);
    char tmp[64];
    snprintf(tmp, sizeof tmp, "%s.new", STATUS);
    FILE *f = fopen(tmp, "w");
    if (!f) return;
    fprintf(f, "checked %lld\nerror %s\n%s", checked, error ? error : "", plan ? plan : "");
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

// rpg update, then the plan: the new status. 0 if the check worked
static int check(void) {
    pthread_mutex_lock(&busy);
    struct text out = { 0, 0, 0 }, plan = { 0, 0, 0 };
    char *up[] = { RPG, "update", 0 };
    int r = run(up, collect, &out);
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
    // the plan even after a failed source: the lists of the others are there
    char *pl[] = { RPG, "upgrade", "--plan", 0 };
    int rp = run(pl, collect, &plan);
    if (rp != 0 && !err[0]) snprintf(err, sizeof err, "%s", plan.s ? plan.s : "rpg upgrade --plan failed");
    err[strcspn(err, "\n")] = 0;
    write_status((long long)time(0), err, rp == 0 ? plan.s : "");
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

static void send_text(rx_handle_t ch, const char *s) {
    size_t l = strlen(s);
    if (l > 60000) l = 60000;
    rx_channel_write(ch, 0, s, (uint32_t)l, 0, 0);
}

struct stream {
    rx_handle_t ch;
};

static void stream_line(const char *line, void *arg) {
    struct stream *st = arg;
    char msg[4200];
    snprintf(msg, sizeof msg, "out %s", line);
    send_text(st->ch, msg);
    syslog(LOG_INFO, "upgrade: %s", line);
}

// May the sender upgrade the system?
static int allowed(const rx_sender_t *who, const char *pw, char *why, size_t cap) {
    if (who->euid == 0) return 1;
    struct passwd *p = getpwuid(who->uid);
    if (!p) {
        snprintf(why, cap, "unknown user %u", who->uid);
        return 0;
    }
    struct group *g = getgrnam("sudo");
    if (!g || !rx_sender_in_group(who, g->gr_gid)) {
        snprintf(why, cap, "%s is not an administrator (group sudo)", p->pw_name);
        return 0;
    }
    pthread_mutex_lock(&udb_lock);
    int ok = pw && udb_check(p->pw_name, pw);
    pthread_mutex_unlock(&udb_lock);
    if (!ok) {
        sleep(2);
        snprintf(why, cap, "wrong password for %s", p->pw_name);
        return 0;
    }
    return 1;
}

static void upgrade(rx_handle_t ch, const rx_sender_t *who, char *pw) {
    char why[160];
    int ok = allowed(who, pw, why, sizeof why);
    if (pw) memset(pw, 0, strlen(pw));
    if (!ok) {
        char msg[200];
        snprintf(msg, sizeof msg, "denied %s", why);
        send_text(ch, msg);
        syslog(LOG_WARNING, "upgrade refused: %s", why);
        return;
    }
    struct passwd *p = getpwuid(who->uid);
    syslog(LOG_INFO, "upgrade asked by %s", p ? p->pw_name : "?");
    pthread_mutex_lock(&busy);
    struct stream st = { ch };
    char *argv[] = { RPG, "upgrade", "-y", 0 };
    int code = run(argv, stream_line, &st);
    last_upgrade = (long long)time(0);
    last_upgrade_code = code;
    pthread_mutex_unlock(&busy);
    check();   // what is left
    char msg[32];
    snprintf(msg, sizeof msg, "done %d", code);
    send_text(ch, msg);
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
        } else if (!strcmp(msg, "upgrade")) {
            upgrade(ch, &who, rest);
        } else {
            send_text(ch, "error status, check or upgrade");
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
