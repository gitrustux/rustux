// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// sudo, su, passwd (step 17): clients of authd (/svc/auth), which checks
// who asks, prompts on this terminal, and runs the program (or changes
// the password). One program; the name it is run by says which.
//
//   sudo [-u USER] [-i | -s] [-S] CMD [ARG...]   CMD as USER (root)
//   sudo -l                                        may I use sudo at all?
//   su [-] [-l] [USER] [-c CMD]                    USER's shell (root)
//   passwd [USER]                                  change a password

#include <rustux/auth.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern char **environ;

static const char *prog = "sudo";

static int usage(void) {
    if (!strcmp(prog, "su")) fprintf(stderr, "usage: su [-] [-l] [USER] [-c COMMAND]\n");
    else if (!strcmp(prog, "passwd")) fprintf(stderr, "usage: passwd [USER]\n");
    else fprintf(stderr, "usage: sudo [-u USER] [-i | -s] [-S] COMMAND [ARG...]\n       sudo -l\n");
    return 1;
}

static int call(uint32_t op, uint32_t flags, const char *user, int argc, char **argv) {
    rx_handle_t ch;
    int r = rx_service_connect("/svc/auth", &ch);
    if (r < 0) {
        fprintf(stderr, "%s: the authentication service is not running (%s)\n", prog, strerror(-r));
        return 1;
    }
    char cwd[4096];
    if (!getcwd(cwd, sizeof cwd)) strcpy(cwd, "/");
    int envc = 0;
    while (op == RX_AUTH_OP_RUN && environ && environ[envc]) envc++;
    size_t len = sizeof(rx_auth_run_t) + strlen(user) + 1 + strlen(cwd) + 1;
    for (int i = 0; i < argc; i++) len += strlen(argv[i]) + 1;
    for (int i = 0; i < envc; i++) len += strlen(environ[i]) + 1;
    if (len > 65000) {
        fprintf(stderr, "%s: the arguments and environment are too long\n", prog);
        return 1;
    }
    char *msg = malloc(len), *p;
    if (!msg) return 1;
    rx_auth_run_t rq = { op, flags, (uint32_t)argc, (uint32_t)envc, { 0, 0, 0 } };
    rx_handle_t hs[3];
    uint32_t nh = 0;
    for (int fd = 0; fd < 3 && op != RX_AUTH_OP_CHECK; fd++) {
        int kind;
        rx_handle_t h = rx_fd_export(fd, &kind);
        if (!h) break;
        hs[nh] = h;
        rq.kinds[nh++] = (uint32_t)kind;
    }
    memcpy(msg, &rq, sizeof rq);
    p = msg + sizeof rq;
    p = stpcpy(p, user) + 1;
    p = stpcpy(p, cwd) + 1;
    for (int i = 0; i < argc; i++) p = stpcpy(p, argv[i]) + 1;
    for (int i = 0; i < envc; i++) p = stpcpy(p, environ[i]) + 1;
    fflush(stdout);
    if (rx_channel_write(ch, 0, msg, (uint32_t)len, hs, nh) != RX_OK) {
        fprintf(stderr, "%s: cannot reach the authentication service\n", prog);
        return 1;
    }
    free(msg);
    // The reply comes when it is done (Ctrl+C ends us, and authd then ends it)
    char rep[12];
    uint32_t nb = 0;
    for (;;) {
        rx_status_t s = rx_channel_read(ch, 0, rep, 0, sizeof rep, 0, &nb, 0);
        if (s == RX_ERR_SHOULD_WAIT) {
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
            continue;
        }
        if (s != RX_OK || nb < 4) {
            fprintf(stderr, "%s: the authentication service went away\n", prog);
            return 1;
        }
        break;
    }
    int32_t st;
    memcpy(&st, rep, 4);
    if (st < 0) return 1;
    if (op != RX_AUTH_OP_RUN) return 0;
    int64_t code = 0;
    if (nb >= 12) memcpy(&code, rep + 4, 8);
    return code < 0 || code > 255 ? 1 : (int)code;
}

int main(int argc, char **argv) {
    const char *base = strrchr(argv[0], '/') ? strrchr(argv[0], '/') + 1 : argv[0];
    if (!strcmp(base, "su") || !strcmp(base, "passwd")) prog = base;
    if (!strcmp(prog, "passwd")) {
        if (argc > 2 || (argc == 2 && argv[1][0] == '-')) return usage();
        return call(RX_AUTH_OP_PASSWD, 0, argc == 2 ? argv[1] : "", 0, 0);
    }
    if (!strcmp(prog, "su")) {
        uint32_t flags = 0;
        const char *user = "root", *cmd = 0;
        int i = 1;
        for (; i < argc; i++) {
            if (!strcmp(argv[i], "-") || !strcmp(argv[i], "-l") || !strcmp(argv[i], "--login")) flags |= RX_AUTH_LOGIN;
            else if (!strcmp(argv[i], "-c") && i + 1 < argc) cmd = argv[++i];
            else if (argv[i][0] == '-') return usage();
            else break;
        }
        if (i < argc) user = argv[i++];
        if (i < argc && !strcmp(argv[i], "-c") && i + 1 < argc) { cmd = argv[i + 1]; i += 2; }
        if (i != argc) return usage();
        char *args[2] = { "-c", (char *)cmd };
        return call(RX_AUTH_OP_RUN, flags, user, cmd ? 2 : 0, args);
    }
    uint32_t flags = RX_AUTH_SUDO;
    const char *user = "root";
    int i = 1;
    for (; i < argc && argv[i][0] == '-'; i++) {
        if (!strcmp(argv[i], "--")) { i++; break; }
        if (!strcmp(argv[i], "-u") && i + 1 < argc) user = argv[++i];
        else if (!strcmp(argv[i], "-i")) flags |= RX_AUTH_SHELL | RX_AUTH_LOGIN;
        else if (!strcmp(argv[i], "-s")) flags |= RX_AUTH_SHELL;
        else if (!strcmp(argv[i], "-S")) flags |= RX_AUTH_STDIN;
        else if (!strcmp(argv[i], "-l")) {
            int r = call(RX_AUTH_OP_CHECK, flags, user, 0, 0);
            printf(r == 0 ? "You may run commands as %s with sudo.\n" : "You may not use sudo (as %s).\n", user);
            return r;
        } else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) {
            usage();
            return 0;
        } else {
            return usage();
        }
    }
    if (i == argc && !(flags & RX_AUTH_SHELL)) return usage();
    return call(RX_AUTH_OP_RUN, flags, user, argc - i, argv + i);
}
