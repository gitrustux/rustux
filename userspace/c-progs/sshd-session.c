// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// sshd-session: the part of the SSH server (sshd, network step D5,
// docs/NETWORK.md) that knows users, as login does (userdb.h). sshd runs
// it as root:
//
//   sshd-session --check USER        is the password on stdin USER's?
//                                    exit 0 yes, 1 no, 2 the account is
//                                    locked or has no password
//   sshd-session USER [COMMAND]      run USER's shell (with -c COMMAND),
//                                    acting as USER, on this program's
//                                    stdin, stdout and stderr (a
//                                    pseudo-terminal or pipes); exit with
//                                    its exit code
//
// The environment: the login's (HOME, USER, LOGNAME, SHELL, PATH, PWD),
// TERM if a pseudo-terminal was asked for, and SSH_CONNECTION/SSH_CLIENT
// from sshd.

#include <errno.h>
#include <pwd.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "userdb.h"

int main(int argc, char **argv) {
    if (geteuid() != 0) {
        fprintf(stderr, "sshd-session: only root may run this (sshd does)\n");
        return 3;
    }
    if (argc == 3 && !strcmp(argv[1], "--check")) {
        char pass[512];
        if (!fgets(pass, sizeof pass, stdin)) return 1;
        pass[strcspn(pass, "\n")] = 0;
        const char *h = udb_hash(argv[2]);
        int r;
        if (!h || !getpwnam(argv[2])) r = (udb_check(argv[2], pass), 1);  // as slow for no such user
        else if (!*h || udb_locked(argv[2])) r = 2;                           // no password, or locked: no SSH
        else r = udb_check(argv[2], pass) ? 0 : 1;
        memset(pass, 0, sizeof pass);
        return r;
    }
    if (argc < 2 || argc > 3) {
        fprintf(stderr, "usage: sshd-session --check USER | USER [COMMAND]\n");
        return 3;
    }
    struct passwd *pw = getpwnam(argv[1]);
    if (!pw) {
        fprintf(stderr, "sshd-session: no user %s\n", argv[1]);
        return 3;
    }
    rx_creds_t c;
    udb_creds(pw, &c);
    char **base_env = udb_login_env(pw, getenv("TERM"));
    char *env[16];
    int n = 0;
    for (; base_env[n] && n < 12; n++) env[n] = base_env[n];
    static char conn[160], client[160];
    if (getenv("SSH_CONNECTION")) {
        snprintf(conn, sizeof conn, "SSH_CONNECTION=%s", getenv("SSH_CONNECTION"));
        env[n++] = conn;
    }
    if (getenv("SSH_CLIENT")) {
        snprintf(client, sizeof client, "SSH_CLIENT=%s", getenv("SSH_CLIENT"));
        env[n++] = client;
    }
    env[n] = 0;
    // In the user's home, as a login
    struct stat st;
    if (stat(pw->pw_dir, &st) == 0 && S_ISDIR(st.st_mode)) chdir(pw->pw_dir);
    else chdir("/");
    const char *shell = *pw->pw_shell ? pw->pw_shell : "/bin/shell";
    const char *name = strrchr(shell, '/') ? strrchr(shell, '/') + 1 : shell;
    char *args[4];
    int k = 0;
    args[k++] = (char *)name;
    if (argc == 3) {
        args[k++] = "-c";
        args[k++] = argv[2];
    } else {
        // A login shell shows the message of the day, as login does
        FILE *m = fopen("/etc/motd", "r");
        if (m) {
            char line[256];
            while (fgets(line, sizeof line, m)) fputs(line, stdout);
            fclose(m);
            fflush(stdout);
        }
    }
    args[k] = 0;
    rx_handle_t proc;
    rx_status_t r = rx_spawn_as(rx_job_default, &c, shell, args, env, RX_SPAWN_CLONE_ALL, 0, 0, &proc);
    if (r != RX_OK) {
        fprintf(stderr, "sshd-session: cannot start %s: %s\n", shell, strerror(errno));
        return 127;
    }
    return (int)rx_wait_process(proc);
}
