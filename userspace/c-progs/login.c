// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// login: sign in on the console (step 17). init runs it on the screen
// (the only one: the getty of a one-screen system) and again when it ends.
//
//   login           asks for a user name and password (three tries)
//   login -f USER   no password (init: autologin, from /etc/autologin or
//                   the kernel command line's autologin=USER)
//
// It first sets the console's colours (/etc/console-theme: Dracula by
// default, or Rustica; `theme`). Then, as that user: /etc/motd, and the user's shell (from /etc/passwd)
// in the home directory with HOME, USER, LOGNAME, SHELL, PATH set. login
// waits for it and ends with its exit code. Only root may run login.

#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "consoletheme.h"
#include "userdb.h"

static int run_session(struct passwd *pw) {
    rx_creds_t c;
    udb_creds(pw, &c);
    char **env = udb_login_env(pw, getenv("TERM"));
    struct stat st;
    if (stat(pw->pw_dir, &st) < 0 || !S_ISDIR(st.st_mode)) printf("No directory %s: logging in with HOME=/\n", pw->pw_dir);
    FILE *m = fopen("/etc/motd", "r");
    if (m) {
        char line[256];
        while (fgets(line, sizeof line, m)) fputs(line, stdout);
        fclose(m);
    }
    fflush(stdout);
    const char *shell = *pw->pw_shell ? pw->pw_shell : "/bin/shell";
    const char *base = strrchr(shell, '/') ? strrchr(shell, '/') + 1 : shell;
    char *argv[] = { (char *)base, 0 };
    // init's control channel (keyboard layout) goes on to the shell, and the
    // system job (ps, kill): all of it for root, to look at for others
    rx_spawn_handle_t hs[2];
    uint32_t nh = 0;
    rx_handle_t ctl = rx_take_startup_handle(RX_PA_USER0), sys = rx_take_startup_handle(RX_PA_JOB_SYSTEM), s2 = 0;
    if (ctl) hs[nh++] = (rx_spawn_handle_t){ RX_PA_USER0, ctl };
    if (sys && rx_handle_duplicate(sys, c.euid == 0 ? RX_RIGHT_SAME_RIGHTS : RX_RIGHTS_LOOK, &s2) == RX_OK)
        hs[nh++] = (rx_spawn_handle_t){ RX_PA_JOB_SYSTEM, s2 };
    rx_handle_t proc;
    rx_status_t r = rx_spawn_as(rx_job_default, &c, shell, argv, env, RX_SPAWN_CLONE_ALL, hs, nh, &proc);
    if (r != RX_OK) {
        fprintf(stderr, "login: cannot start %s: %s\n", shell, strerror(errno));
        sleep(2);
        return 1;
    }
    return (int)rx_wait_process(proc);
}

int main(int argc, char **argv) {
    if (geteuid() != 0) {
        fprintf(stderr, "login: only root may run login (it is started by init)\n");
        return 1;
    }
    {
        char name[32];
        console_theme_saved(name, sizeof name);
        // Dracula is the console's own: only another one needs the sequences
        if (strcmp(name, "dracula")) console_theme_apply(stdout, console_theme_find(name));
    }
    if (argc == 3 && !strcmp(argv[1], "-f")) {
        struct passwd *pw = getpwnam(argv[2]);
        if (!pw) {
            fprintf(stderr, "login: no user %s (for autologin)\n", argv[2]);
            sleep(2);
        } else {
            return run_session(pw);
        }
    } else if (argc != 1) {
        fprintf(stderr, "usage: login [-f USER]\n");
        return 2;
    }
    char host[65] = "rustica";
    gethostname(host, sizeof host);
    printf("\n");
    for (int tries = 0; tries < 3; tries++) {
        char name[64];
        printf("%s login: ", host);
        fflush(stdout);
        if (!fgets(name, sizeof name, stdin)) return 1;
        name[strcspn(name, "\r\n")] = 0;
        if (!name[0]) { tries--; continue; }
        const char *pass = getpass("Password: ");
        struct passwd *pw = getpwnam(name);
        int ok = pw && !udb_locked(name) ? udb_check(name, pass ? pass : "") : (udb_check(name, ""), 0);
        if (pass) memset((char *)pass, 0, strlen(pass));
        if (ok) return run_session(pw);
        sleep(2);
        printf("Login incorrect\n\n");
    }
    return 1;
}
