// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// authd: running programs as another user, and changing passwords (step
// 17). init starts it as root; it serves /svc/auth (<rustux/auth.h>).
// Rustux has no setuid programs: su, sudo and passwd are clients that ask
// authd, which knows who asks from the kernel (the sender of each
// request) and prompts on the caller's own terminal.
//
// - su [-] [USER] [-c CMD]: the target's password (none for root), then
//   the target's shell (USER: root by default).
// - sudo [-u USER] [-i|-s] CMD...: /etc/sudoers decides; the caller's own
//   password (remembered for 5 minutes), unless NOPASSWD. The environment
//   is reset (PATH=/bin, HOME, USER, LOGNAME, SHELL of the target; TERM;
//   SUDO_USER, SUDO_UID, SUDO_GID, SUDO_COMMAND).
// - passwd [USER]: a user's own password (after the current one), or root
//   any user's; /etc/shadow (SHA-512).
//
// /etc/sudoers: lines "WHO HOST=(RUNAS) [NOPASSWD:] COMMANDS"; WHO a user
// or %group; HOST is not checked (ALL); RUNAS ALL, ALL:ALL or user names
// (default root); COMMANDS ALL or full paths, comma-separated. Defaults
// lines and #includedir are ignored. Each use is logged in
// /var/log/auth.log and the system log (logs -u authd).

#include <pthread.h>
#include <stdarg.h>
#include <rustux/auth.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <syslog.h>
#include <termios.h>
#include <unistd.h>

#include "userdb.h"

#define MAX_TIMESTAMPS 64
#define TIMESTAMP_SECONDS 300

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t sys_job;   // the system job from init (ps, kill)
static struct { uint32_t uid; time_t when; } stamps[MAX_TIMESTAMPS];

static int stamp_fresh(uint32_t uid) {
    time_t now = time(0);
    for (int i = 0; i < MAX_TIMESTAMPS; i++)
        if (stamps[i].when && stamps[i].uid == uid && now - stamps[i].when < TIMESTAMP_SECONDS) return 1;
    return 0;
}

static void stamp_set(uint32_t uid) {
    int k = 0;
    for (int i = 0; i < MAX_TIMESTAMPS; i++) {
        if (stamps[i].uid == uid) { k = i; break; }
        if (stamps[i].when < stamps[k].when) k = i;
    }
    stamps[k].uid = uid;
    stamps[k].when = time(0);
}

static void log_line(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void log_line(const char *fmt, ...) {
    static pthread_mutex_t log_lock = PTHREAD_MUTEX_INITIALIZER;
    char msg[600];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(msg, sizeof msg, fmt, ap);
    va_end(ap);
    pthread_mutex_lock(&log_lock);
    syslog(LOG_AUTHPRIV | LOG_NOTICE, "%s", msg); // the system log too (step 18)
    FILE *f = fopen("/var/log/auth.log", "a");
    if (f) {
        char when[32];
        time_t t = time(0);
        strftime(when, sizeof when, "%Y-%m-%d %H:%M:%S", localtime(&t));
        fprintf(f, "%s %s\n", when, msg);
        fclose(f);
    }
    pthread_mutex_unlock(&log_lock);
}

// ---- the caller's terminal ----

struct term { int in, out; };   // our fds for the caller's stdin and stderr

static void say(struct term *t, const char *s) {
    if (t->out >= 0) write(t->out, s, strlen(s));
}

// One line from the caller's stdin, without echo when it is a terminal
static int ask(struct term *t, const char *prompt, char *buf, size_t cap, int echo) {
    struct termios old, quiet;
    int tty = t->in >= 0 && tcgetattr(t->in, &old) == 0;
    if (prompt) say(t, prompt);
    if (tty && !echo) {
        quiet = old;
        quiet.c_lflag &= ~(tcflag_t)ECHO;
        tcsetattr(t->in, TCSAFLUSH, &quiet);
    }
    size_t n = 0;
    char ch;
    ssize_t r = -1;
    while (t->in >= 0 && (r = read(t->in, &ch, 1)) == 1 && ch != '\n' && ch != '\r')
        if (n + 1 < cap) buf[n++] = ch;
    buf[n] = 0;
    if (tty && !echo) {
        tcsetattr(t->in, TCSAFLUSH, &old);
        say(t, "\n");
    }
    return r == 1 || n ? 0 : -1;
}

// Ask for `user`'s password (three tries); 1 if given
static int authenticate(struct term *t, const char *user, const char *prompt_fmt, int from_stdin) {
    for (int tries = 0; tries < 3; tries++) {
        char prompt[128], pw[256];
        snprintf(prompt, sizeof prompt, prompt_fmt, user);
        if (ask(t, from_stdin ? 0 : prompt, pw, sizeof pw, 0) < 0) return 0;
        pthread_mutex_lock(&lock);
        int ok = udb_check(user, pw);
        pthread_mutex_unlock(&lock);
        memset(pw, 0, sizeof pw);
        if (ok) return 1;
        sleep(2);
        say(t, "Sorry, try again.\n");
        if (from_stdin) return 0;
    }
    return 0;
}

// ---- sudoers ----

static int in_group_named(const rx_sender_t *who, const char *name) {
    struct group *g = getgrnam(name);
    return g && rx_sender_in_group(who, g->gr_gid);
}

static int list_has(const char *list, const char *item) {
    char buf[512];
    snprintf(buf, sizeof buf, "%s", list);
    for (char *s = strtok(buf, ", \t"); s; s = strtok(0, ", \t"))
        if (!strcmp(s, "ALL") || !strcmp(s, item)) return 1;
    return 0;
}

// May the caller (`me`, `who`) run `cmd` (a full path; 0: anything) as
// `target`? *nopasswd: without a password
static int sudo_allowed(const char *me, const rx_sender_t *who, const char *target, const char *cmd, int *nopasswd) {
    FILE *f = fopen("/etc/sudoers", "r");
    if (!f) return 0;
    char line[1024];
    int allowed = 0;
    while (fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        char *p = line;
        while (*p == ' ' || *p == '\t') p++;
        if (!*p || *p == '#' || !strncmp(p, "Defaults", 8)) continue;
        char whom[64];
        size_t n = strcspn(p, " \t");
        if (n >= sizeof whom) continue;
        memcpy(whom, p, n);
        whom[n] = 0;
        p += n;
        int match = whom[0] == '%' ? in_group_named(who, whom + 1) : !strcmp(whom, me) || !strcmp(whom, "ALL");
        if (!match) continue;
        char *eq = strchr(p, '=');
        if (!eq) continue;
        p = eq + 1;
        while (*p == ' ') p++;
        char runas[256] = "root";
        if (*p == '(') {
            char *close = strchr(p, ')');
            if (!close) continue;
            size_t k = (size_t)(close - p - 1);
            if (k >= sizeof runas) k = sizeof runas - 1;
            memcpy(runas, p + 1, k);
            runas[k] = 0;
            runas[strcspn(runas, ":")] = 0;   // the group part is not used
            p = close + 1;
        }
        while (*p == ' ') p++;
        int np = 0;
        if (!strncmp(p, "NOPASSWD:", 9)) { np = 1; p += 9; }
        else if (!strncmp(p, "PASSWD:", 7)) p += 7;
        while (*p == ' ') p++;
        if (!list_has(runas, target)) continue;
        if (cmd && !list_has(p, cmd)) continue;
        allowed = 1;
        *nopasswd = np;   // the last matching line wins, as in sudo
    }
    fclose(f);
    return allowed;
}

// A command's full path: as given if it has a '/', else in /bin
static int resolve(const char *name, char *out, size_t cap) {
    if (strchr(name, '/')) snprintf(out, cap, "%s", name);
    else snprintf(out, cap, "/bin/%s", name);
    struct stat st;
    return stat(out, &st) == 0 && S_ISREG(st.st_mode) ? 0 : -1;
}

// ---- one request ----

struct client { rx_handle_t ch; };

static void reply(rx_handle_t ch, int32_t st, int64_t code) {
    char buf[12];
    memcpy(buf, &st, 4);
    memcpy(buf + 4, &code, 8);
    rx_channel_write(ch, 0, buf, st == 0 ? 12 : 4, 0, 0);
}

// The strings after the header: user, cwd, args, env
static int strings(char *p, size_t n, char **v, int want) {
    int k = 0;
    size_t off = 0;
    while (k < want && off < n) {
        v[k++] = p + off;
        size_t l = strnlen(p + off, n - off);
        if (l == n - off) return -1;
        off += l + 1;
    }
    return k == want ? 0 : -1;
}

static int bind_handle(rx_handle_t h, uint32_t kind) {
    if (!h || !kind) return -1;
    int fd = open("/dev/null", O_RDONLY);
    if (fd < 0) {
        rx_handle_close(h);
        return -1;
    }
    rx_fd_bind(fd, (int)kind, h);
    return fd;
}

static int do_passwd(rx_handle_t ch, const rx_sender_t *who, const char *me, const char *target, struct term *t) {
    if (!*target) target = me;
    pthread_mutex_lock(&lock);
    int exists = getpwnam(target) != 0;
    const char *h = udb_hash(target);
    int has_pw = h && *h && *h != '!' && *h != '*';
    pthread_mutex_unlock(&lock);
    if (!exists) {
        say(t, "passwd: no such user\n");
        return -ENOENT;
    }
    if (who->uid != 0 && strcmp(target, me)) {
        say(t, "passwd: you may only change your own password\n");
        return -EPERM;
    }
    char head[96];
    snprintf(head, sizeof head, "Changing password for %s.\n", target);
    say(t, head);
    if (who->uid != 0 && has_pw && !authenticate(t, target, "Current password: ", 0)) {
        say(t, "passwd: Authentication token manipulation error\n");
        log_line("passwd: %s: authentication failure", me);
        return -EACCES;
    }
    char a[256], b[256];
    if (ask(t, "New password: ", a, sizeof a, 0) < 0 || ask(t, "Retype new password: ", b, sizeof b, 0) < 0) return -EIO;
    int same = !strcmp(a, b), empty = !a[0];
    int r = 0;
    if (!same) say(t, "Sorry, passwords do not match.\npasswd: password unchanged\n");
    else if (empty) say(t, "No password has been supplied.\npasswd: password unchanged\n");
    else {
        pthread_mutex_lock(&lock);
        r = udb_set_password(target, a);
        pthread_mutex_unlock(&lock);
        say(t, r == 0 ? "passwd: password updated successfully\n" : "passwd: could not write /etc/shadow\n");
        if (r == 0) log_line("passwd: %s changed the password of %s", me, target);
    }
    memset(a, 0, sizeof a);
    memset(b, 0, sizeof b);
    (void)ch;
    return !same || empty ? -EINVAL : r;
}

static void *serve_client(void *arg) {
    struct client *cl = arg;
    rx_handle_t ch = cl->ch;
    free(cl);
    char *msg = malloc(65536);
    if (!msg) {
        rx_handle_close(ch);
        return 0;
    }
    for (;;) {
        rx_handle_t hs[3] = { 0, 0, 0 };
        uint32_t nb = 0, nh = 0;
        rx_sender_t who;
        rx_status_t s = rx_channel_read_creds(ch, msg, hs, 65535, 3, &nb, &nh, &who);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t pend = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
            if (!(pend & RX_CHANNEL_READABLE)) break;
            continue;
        }
        if (s != RX_OK) break;
        rx_auth_run_t rq;
        if (nb < sizeof rq) {
            for (uint32_t i = 0; i < nh; i++) rx_handle_close(hs[i]);
            reply(ch, -EINVAL, 0);
            continue;
        }
        memcpy(&rq, msg, sizeof rq);
        msg[nb] = 0;
        int want = 2 + (int)rq.argc + (int)rq.envc;
        char **v = calloc((size_t)want + 1, sizeof *v);
        if (!v || rq.argc > 1000 || rq.envc > 1000 || strings(msg + sizeof rq, nb - sizeof rq, v, want) < 0) {
            free(v);
            for (uint32_t i = 0; i < nh; i++) rx_handle_close(hs[i]);
            reply(ch, -EINVAL, 0);
            continue;
        }
        const char *target = v[0], *cwd = v[1];
        char **args = v + 2, **env = v + 2 + rq.argc;
        struct term t = { -1, -1 };
        int fds[3] = { -1, -1, -1 };
        for (uint32_t i = 0; i < 3; i++) fds[i] = i < nh ? bind_handle(hs[i], rq.kinds[i]) : -1;
        t.in = fds[0];
        t.out = fds[2] >= 0 ? fds[2] : fds[1];
        // Who asks: by the real user id
        char me[64];
        pthread_mutex_lock(&lock);
        struct passwd *mpw = getpwuid(who.uid);
        snprintf(me, sizeof me, "%s", mpw ? mpw->pw_name : "?");
        pthread_mutex_unlock(&lock);
        int st = 0;
        int64_t code = 0;
        if (rq.op == RX_AUTH_OP_PASSWD) {
            st = do_passwd(ch, &who, me, target, &t);
        } else if (rq.op == RX_AUTH_OP_RUN || rq.op == RX_AUTH_OP_CHECK) {
            int sudo = (rq.flags & RX_AUTH_SUDO) != 0, login = (rq.flags & RX_AUTH_LOGIN) != 0;
            if (!*target) target = "root";
            pthread_mutex_lock(&lock);
            struct passwd *tp = getpwnam(target), tpw;
            char tbuf[4][300];
            if (tp) {
                tpw = *tp;
                snprintf(tbuf[0], 300, "%s", tp->pw_name);
                snprintf(tbuf[1], 300, "%s", tp->pw_dir);
                snprintf(tbuf[2], 300, "%s", *tp->pw_shell ? tp->pw_shell : "/bin/shell");
                tpw.pw_name = tbuf[0];
                tpw.pw_dir = tbuf[1];
                tpw.pw_shell = tbuf[2];
            }
            pthread_mutex_unlock(&lock);
            // What to run: sudo's command (or the target's shell with -i/-s), su's shell
            char path[300] = "", joined[4096];
            char *argv[1100];
            int ac = 0;
            if (!tp) {
                say(&t, sudo ? "sudo: unknown user\n" : "su: user does not exist\n");
                st = -ENOENT;
            } else if (sudo && rq.argc > 0 && !(rq.flags & RX_AUTH_SHELL)) {
                if (resolve(args[0], path, sizeof path) < 0) {
                    char m[400];
                    snprintf(m, sizeof m, "sudo: %s: command not found\n", args[0]);
                    say(&t, m);
                    st = -ENOENT;
                }
                for (uint32_t i = 0; i < rq.argc && ac < 1098; i++) argv[ac++] = args[i];
            } else {
                // A shell: su's (with its arguments: -c CMD), or sudo -s/-i (CMD run by it)
                snprintf(path, sizeof path, "%s", tpw.pw_shell);
                const char *base = strrchr(path, '/') ? strrchr(path, '/') + 1 : path;
                argv[ac++] = (char *)base;
                if (!sudo) {
                    for (uint32_t i = 0; i < rq.argc && ac < 1098; i++) argv[ac++] = args[i];
                } else if (rq.argc > 0) {
                    joined[0] = 0;
                    for (uint32_t i = 0; i < rq.argc; i++) {
                        if (i) strncat(joined, " ", sizeof joined - strlen(joined) - 1);
                        strncat(joined, args[i], sizeof joined - strlen(joined) - 1);
                    }
                    argv[ac++] = "-c";
                    argv[ac++] = joined;
                }
            }
            argv[ac] = 0;
            // May they?
            if (st == 0) {
                int nopasswd = 0;
                if (sudo) {
                    pthread_mutex_lock(&lock);
                    int ok = who.uid == 0 || sudo_allowed(me, &who, target, rq.op == RX_AUTH_OP_CHECK ? 0 : path, &nopasswd);
                    pthread_mutex_unlock(&lock);
                    if (!ok) {
                        char m[300];
                        snprintf(m, sizeof m, "%s is not in the sudoers file (or may not run that). This incident has been logged.\n", me);
                        say(&t, rq.op == RX_AUTH_OP_CHECK ? "" : m);
                        log_line("sudo: %s : NOT ALLOWED ; USER=%s ; COMMAND=%s", me, target, path);
                        st = -EPERM;
                    } else if (rq.op == RX_AUTH_OP_RUN && who.uid != 0 && !nopasswd && !stamp_fresh(who.uid)) {
                        if (authenticate(&t, me, "[sudo] password for %s: ", (rq.flags & RX_AUTH_STDIN) != 0)) {
                            stamp_set(who.uid);
                        } else {
                            say(&t, "sudo: incorrect password\n");
                            log_line("sudo: %s : authentication failure ; USER=%s ; COMMAND=%s", me, target, path);
                            st = -EACCES;
                        }
                    }
                } else if (who.uid != 0) {
                    if (!authenticate(&t, target, "Password: ", 0)) {
                        say(&t, "su: Authentication failure\n");
                        log_line("su: %s : authentication failure ; USER=%s", me, target);
                        st = -EACCES;
                    }
                }
            }
            if (st == 0 && rq.op == RX_AUTH_OP_RUN) {
                // The environment
                char *e[1100];
                int ne = 0;
                char ebuf[12][400];
                if (sudo || login) {
                    char **le = udb_login_env(&tpw, 0);
                    for (int i = 0; le[i] && ne < 1000; i++) {
                        if (!strncmp(le[i], "PWD=", 4) && !login) continue;
                        snprintf(ebuf[ne], 400, "%s", le[i]);
                        e[ne] = ebuf[ne];
                        ne++;
                    }
                    for (uint32_t i = 0; i < rq.envc; i++)
                        if (!strncmp(env[i], "TERM=", 5)) e[ne++] = env[i];
                    if (!login) {
                        snprintf(ebuf[8], 400, "PWD=%s", cwd);
                        e[ne++] = ebuf[8];
                    }
                    if (sudo) {
                        snprintf(ebuf[9], 400, "SUDO_USER=%s", me);
                        snprintf(ebuf[10], 400, "SUDO_UID=%u", who.uid);
                        snprintf(ebuf[11], 400, "SUDO_GID=%u", who.gid);
                        e[ne++] = ebuf[9];
                        e[ne++] = ebuf[10];
                        e[ne++] = ebuf[11];
                    }
                } else {
                    // su: the caller's environment; HOME, SHELL and (but for root) USER, LOGNAME the target's
                    for (uint32_t i = 0; i < rq.envc && ne < 1090; i++) {
                        if (!strncmp(env[i], "HOME=", 5) || !strncmp(env[i], "SHELL=", 6) || !strncmp(env[i], "USER=", 5) ||
                            !strncmp(env[i], "LOGNAME=", 8) || !strncmp(env[i], "PWD=", 4))
                            continue;
                        e[ne++] = env[i];
                    }
                    snprintf(ebuf[0], 400, "HOME=%s", tpw.pw_dir);
                    snprintf(ebuf[1], 400, "SHELL=%s", tpw.pw_shell);
                    snprintf(ebuf[2], 400, "USER=%s", tpw.pw_name);
                    snprintf(ebuf[3], 400, "LOGNAME=%s", tpw.pw_name);
                    snprintf(ebuf[4], 400, "PWD=%s", cwd);
                    for (int i = 0; i < 5; i++) e[ne++] = ebuf[i];
                }
                e[ne] = 0;
                log_line("%s: %s : USER=%s ; PWD=%s ; COMMAND=%s", sudo ? "sudo" : "su", me, target, cwd, path);
                // As the target, in a job of its own (killed if the caller goes)
                rx_creds_t c;
                pthread_mutex_lock(&lock);
                udb_creds(&tpw, &c);
                pthread_mutex_unlock(&lock);
                rx_handle_t job = 0, proc = 0;
                rx_spawn_handle_t sh[4];
                uint32_t n = 0;
                for (int i = 0; i < 3; i++) {
                    int kind;
                    rx_handle_t h = fds[i] >= 0 ? rx_fd_export(fds[i], &kind) : 0;
                    if (h) sh[n++] = (rx_spawn_handle_t){ RX_FD_INFO(i, kind), h };
                }
                rx_handle_t sj = 0;
                if (sys_job && rx_handle_duplicate(sys_job, c.euid == 0 ? RX_RIGHT_SAME_RIGHTS : RX_RIGHTS_LOOK, &sj) == RX_OK)
                    sh[n++] = (rx_spawn_handle_t){ RX_PA_JOB_SYSTEM, sj };
                if (rx_job_create(rx_job_default, 0, &job) != RX_OK) job = 0;
                rx_status_t r = rx_spawn_as(job ? job : rx_job_default, &c, path, argv, e, RX_SPAWN_CLONE_NS, sh, n, &proc);
                if (r != RX_OK) {
                    char m[400];
                    snprintf(m, sizeof m, "%s: %s: %s\n", sudo ? "sudo" : "su", path, strerror(errno));
                    say(&t, m);
                    st = -errno;
                } else {
                    rx_wait_item_t w[2] = { { proc, RX_PROCESS_TERMINATED, 0 }, { ch, RX_PEER_CLOSED, 0 } };
                    rx_object_wait_many(w, 2, RX_TIME_INFINITE);
                    if (!(w[0].pending & RX_PROCESS_TERMINATED)) {
                        rx_task_kill(job ? job : proc);
                        rx_object_wait_one(proc, RX_PROCESS_TERMINATED, RX_TIME_INFINITE, 0);
                    }
                    code = rx_wait_process(proc);
                }
                if (job) rx_handle_close(job);
            }
        } else {
            st = -ENOSYS;
        }
        for (int i = 0; i < 3; i++) if (fds[i] >= 0) close(fds[i]);
        free(v);
        reply(ch, st, code);
    }
    free(msg);
    rx_handle_close(ch);
    return 0;
}

int main(void) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    if (geteuid() != 0) {
        fprintf(stderr, "authd: must run as root (init starts it)\n");
        return 1;
    }
    sys_job = rx_take_startup_handle(RX_PA_JOB_SYSTEM);
    rx_handle_t mine, theirs;
    if (rx_channel_create(0, &mine, &theirs) != RX_OK || rx_service_publish("/svc/auth", theirs) < 0) {
        fprintf(stderr, "authd: cannot publish /svc/auth\n");
        return 1;
    }
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
        pthread_t th;
        if (!cl) { rx_handle_close(h); continue; }
        cl->ch = h;
        if (pthread_create(&th, 0, serve_client, cl) != 0) {
            rx_handle_close(h);
            free(cl);
            continue;
        }
        pthread_detach(th);
    }
}
