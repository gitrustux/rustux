// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// desktop: the desktop session (DESKTOP.md step F6, docs/SESSION.md).
//
//   desktop          (root) start the desktop: asks the service manager to
//                    start the "desktop" unit; Ctrl+Alt+F1 comes back to
//                    the console, Ctrl+Alt+F7 to the desktop
//   desktop --stop   (root) stop it
//   desktop --run    the unit's program, the session manager:
//
// It starts the compositor (with a control channel), then over and over:
// the graphical login (/bin/greeter, run as root; it sends the name and
// password it was given, one line "login\tNAME\tPASSWORD" on its stdout,
// and gets "ok" or "fail" on its stdin), then the session as that user:
// a job of its own with the panel (/bin/panel, the user's HOME, USER,
// PATH...) and everything started from it. When the panel ends (Log out),
// the job is killed, every program of the session with it, and the login
// comes back. The compositor is told whose session it is ("user NAME"),
// for its lock screen.
//
// The first session starts without the login when /etc/autologin names a
// user (the live image: root), as on the console.
//
// The console stays on Ctrl+Alt+F1 throughout. If the compositor ends,
// so does desktop --run (the service manager starts it again).

#include <errno.h>
#include <rustux/auth.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#include "userdb.h"

static rx_handle_t comp_proc, comp_ctl;

static void tell_compositor(const char *msg) {
    rx_channel_write(comp_ctl, 0, msg, (uint32_t)strlen(msg), 0, 0);
}

static int start_compositor(void) {
    rx_handle_t theirs;
    rx_channel_create(0, &comp_ctl, &theirs);
    rx_spawn_handle_t hs[] = { { RX_PA_USER0, theirs } };
    char *argv[] = { "compositor", "-v", 0 };
    if (rx_spawn("/bin/compositor", argv, 0, RX_SPAWN_CLONE_ALL, hs, 1, &comp_proc) != RX_OK) {
        printf("desktop: cannot start /bin/compositor\n");
        return -1;
    }
    // Wait for /svc/window
    for (int i = 0; i < 100; i++) {
        rx_handle_t ch;
        if (rx_service_connect("/svc/window", &ch) == 0) {
            rx_handle_close(ch);
            return 0;
        }
        usleep(100000);
    }
    printf("desktop: the compositor did not start\n");
    return -1;
}

static int compositor_alive(void) {
    uint32_t pend = 0;
    rx_object_wait_one(comp_proc, RX_PROCESS_TERMINATED, 0, &pend);
    return !(pend & RX_PROCESS_TERMINATED);
}

// The graphical login: the name of who logged in (in user), 0 on success
// auto: the user /etc/autologin names (the live system), who may log in
// again without a password ("auto\tNAME"), or ""
static int greet(char *user, size_t usize, const char *autou) {
    int to[2], from[2];
    if (pipe(to) < 0 || pipe(from) < 0) return -1;
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    posix_spawn_file_actions_adddup2(&fa, to[0], 0);
    posix_spawn_file_actions_adddup2(&fa, from[1], 1);
    posix_spawn_file_actions_addclose(&fa, to[1]);
    posix_spawn_file_actions_addclose(&fa, from[0]);
    char *argv[] = { "greeter", autou[0] ? "--auto" : 0, (char *)autou, 0 };
    extern char **environ;
    pid_t pid;
    int r = posix_spawn(&pid, "/bin/greeter", &fa, 0, argv, environ);
    posix_spawn_file_actions_destroy(&fa);
    close(to[0]);
    close(from[1]);
    if (r != 0) {
        printf("desktop: cannot start /bin/greeter\n");
        close(to[1]);
        close(from[0]);
        return -1;
    }
    FILE *in = fdopen(from[0], "r");
    int result = -1;
    char line[256];
    while (in && fgets(line, sizeof line, in)) {
        line[strcspn(line, "\r\n")] = 0;
        char *save, *cmd = strtok_r(line, "\t", &save), *name = strtok_r(0, "\t", &save), *pw = strtok_r(0, "", &save);
        if (cmd && name && !strcmp(cmd, "auto")) {
            if (autou[0] && !strcmp(name, autou) && getpwnam(name)) {
                write(to[1], "ok\n", 3);
                snprintf(user, usize, "%s", name);
                printf("desktop: %s logged in again (/etc/autologin)\n", name);
                result = 0;
                break;
            }
            write(to[1], "fail\n", 5);
            continue;
        }
        if (!cmd || strcmp(cmd, "login") || !name) continue;
        if (!pw) pw = "";
        struct passwd *p = getpwnam(name);
        int ok = p && !udb_locked(name) ? udb_check(name, pw) : (udb_check(name, ""), 0);
        memset(pw, 0, strlen(pw));
        if (ok) {
            write(to[1], "ok\n", 3);
            snprintf(user, usize, "%s", name);
            printf("desktop: %s logged in\n", name);
            result = 0;
            break;
        }
        printf("desktop: a failed login for %s\n", name);
        sleep(1);
        write(to[1], "fail\n", 5);
    }
    if (in) fclose(in);
    close(to[1]);
    int st;
    if (result != 0) kill(pid, SIGKILL);
    waitpid(pid, &st, 0);
    return result;
}

// The session: the panel as the user, in a job of its own
static void session(const char *name) {
    struct passwd *pw = getpwnam(name);
    if (!pw) return;
    rx_creds_t c;
    udb_creds(pw, &c);
    char **env = udb_login_env(pw, 0);
    rx_handle_t job;
    if (rx_job_create(rx_job_default, 0, &job) != RX_OK) {
        printf("desktop: no job for the session\n");
        return;
    }
    char who[80];
    snprintf(who, sizeof who, "user %s", name);
    tell_compositor(who);
    // The session's user may shut down and restart (as with logind's
    // active session): init reads this
    FILE *sf = fopen("/run/desktop-session", "w");
    if (sf) {
        fprintf(sf, "%u\n", (unsigned)pw->pw_uid);
        fclose(sf);
    }
    char *argv[] = { "panel", 0 };
    rx_handle_t proc;
    // In the home directory, like a login
    char cwd[256] = "/";
    getcwd(cwd, sizeof cwd);
    if (chdir(pw->pw_dir) < 0) chdir("/");
    rx_status_t r = rx_spawn_as(job, &c, "/bin/panel", argv, env, RX_SPAWN_CLONE_ALL, 0, 0, &proc);
    chdir(cwd);
    if (r != RX_OK) {
        printf("desktop: cannot start the panel for %s\n", name);
    } else {
        // Until the panel ends (Log out) or the compositor does
        rx_wait_item_t items[2] = { { proc, RX_PROCESS_TERMINATED, 0 }, { comp_proc, RX_PROCESS_TERMINATED, 0 } };
        rx_object_wait_many(items, 2, RX_TIME_INFINITE);
        rx_handle_close(proc);
    }
    // Everything the session started goes with it
    rx_task_kill(job);
    rx_handle_close(job);
    unlink("/run/desktop-session");
    tell_compositor("user");
    printf("desktop: %s logged out\n", name);
}

static int svc_call(uint32_t cmd, const char *unit) {
    rx_handle_t ch;
    if (rx_service_connect("/svc/system", &ch) < 0) {
        fprintf(stderr, "desktop: no service manager\n");
        return 1;
    }
    rx_svc_req_t rq;
    memset(&rq, 0, sizeof rq);
    rq.op = RX_SYSTEM_OP_SVC;
    rq.cmd = cmd;
    snprintf(rq.name, sizeof rq.name, "%s", unit);
    rx_channel_write(ch, 0, &rq, sizeof rq, 0, 0);
    char reply[512];
    uint32_t nb = 0, pend = 0;
    rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
    int32_t st = -EIO;
    if (rx_channel_read(ch, 0, reply, 0, sizeof reply - 1, 0, &nb, 0) == RX_OK && nb >= 4) {
        memcpy(&st, reply, 4);
        reply[nb] = 0;
        if (nb > 4) fputs(reply + 4, stdout);
    }
    rx_handle_close(ch);
    if (st < 0) {
        fprintf(stderr, "desktop: %s\n", strerror(-st));
        return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "--run")) {
        if (geteuid() != 0) {
            fprintf(stderr, "desktop --run: the service manager runs this (as root)\n");
            return 1;
        }
        setvbuf(stdout, 0, _IONBF, 0);
        if (start_compositor() < 0) return 1;
        char auto_user[64] = "";
        FILE *f = fopen("/etc/autologin", "r");
        if (f) {
            if (fgets(auto_user, sizeof auto_user, f)) auto_user[strcspn(auto_user, " \t\r\n")] = 0;
            fclose(f);
        }
        if (auto_user[0] && getpwnam(auto_user)) {
            printf("desktop: %s logged in (/etc/autologin)\n", auto_user);
            session(auto_user);
        }
        for (;;) {
            if (!compositor_alive()) {
                printf("desktop: the compositor ended\n");
                return 1;
            }
            char user[64];
            if (greet(user, sizeof user, auto_user[0] && getpwnam(auto_user) ? auto_user : "") == 0) session(user);
            else sleep(1);
        }
    }
    if (argc > 2 || (argc == 2 && strcmp(argv[1], "--stop"))) {
        fprintf(stderr, "usage: desktop [--stop]\n");
        return 2;
    }
    if (geteuid() != 0) {
        fprintf(stderr, "desktop: only root may start the desktop (sudo desktop)\n");
        return 1;
    }
    if (argc == 2) return svc_call(RX_SVC_STOP, "desktop");
    int r = svc_call(RX_SVC_START, "desktop");
    if (!r) printf("desktop: starting; Ctrl+Alt+F1 comes back here, Ctrl+Alt+F7 to the desktop\n");
    return r;
}
