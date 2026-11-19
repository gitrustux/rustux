// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// updates: the system's updates, through the installer service
// /svc/packages (package step E6). Settings > Updates uses it too.
//
//   updates [status]       what the last check found (--raw: the status lines)
//   updates check          check now (rpg update), then status
//   updates install [-S]   install them (rpg upgrade): root, or an
//                          administrator with their password (asked; -S:
//                          one line on stdin, as sudo -S)
//
// Exit status: 0; 1 a failure; for status and check, 100 when there are
// updates (for scripts).

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <rustux/service.h>
#include <rustux/syscalls.h>

static rx_handle_t ch;

static int ask(const char *req) {
    if (rx_service_connect("/svc/packages", &ch) < 0) {
        fprintf(stderr, "updates: the installer service is not running (svc status packagesd)\n");
        return -1;
    }
    return rx_channel_write(ch, 0, req, (uint32_t)strlen(req), 0, 0) == RX_OK ? 0 : -1;
}

// The next reply (NUL-terminated in buf); -1 when the service went
static int reply(char *buf, size_t cap) {
    for (;;) {
        uint32_t nb = 0, nh = 0;
        rx_status_t s = rx_channel_read(ch, 0, buf, 0, (uint32_t)cap - 1, 0, &nb, &nh);
        if (s == RX_OK) {
            buf[nb] = 0;
            return (int)nb;
        }
        if (s != RX_ERR_SHOULD_WAIT) return -1;
        uint32_t pend = 0;
        rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
        if (!(pend & RX_CHANNEL_READABLE)) return -1;
    }
}

static const char *size_text(long long n) {
    static char b[32];
    if (n < 1024 * 1024) snprintf(b, sizeof b, "%lld KiB", (n + 1023) / 1024);
    else snprintf(b, sizeof b, "%.1f MiB", n / 1048576.0);
    return b;
}

// The status, for people; the number of packages to upgrade
static int show(char *st, int raw) {
    if (raw) {
        fputs(st, stdout);
    }
    long long checked = 0, total = 0;
    int n = 0;
    char err[300] = "";
    char *save = 0;
    for (char *l = strtok_r(st, "\n", &save); l; l = strtok_r(0, "\n", &save)) {
        if (!strncmp(l, "checked ", 8)) checked = atoll(l + 8);
        else if (!strncmp(l, "error ", 6)) snprintf(err, sizeof err, "%s", l + 6);
        else if (!strncmp(l, "step ", 5)) {
            char line[512], *f[5], *s2 = 0;
            snprintf(line, sizeof line, "%s", l + 5);
            int k = 0;
            for (char *w = strtok_r(line, " ", &s2); w && k < 5; w = strtok_r(0, " ", &s2)) f[k++] = w;
            if (k == 5) {
                const char *name = f[0], *from = f[1], *to = f[2], *why = f[4];
                long long size = atoll(f[3]);
                n++;
                total += size;
                if (!raw) {
                    if (!strcmp(from, "-")) printf("  %-28s new %s (%s)%s%s\n", name, to, size_text(size), strcmp(why, "-") ? ", for " : "", strcmp(why, "-") ? why : "");
                    else printf("  %-28s %s -> %s (%s)\n", name, from, to, size_text(size));
                }
            }
        } else if (!strncmp(l, "kept ", 5) && !raw) {
            printf("  kept back: %s\n", l + 5);
        }
    }
    if (raw) return n;
    if (!checked) printf("Not checked yet (updates check).\n");
    else {
        char when[64];
        time_t t = (time_t)checked;
        strftime(when, sizeof when, "%Y-%m-%d %H:%M", localtime(&t));
        printf("Checked %s%s%s\n", when, err[0] ? ": " : "", err);
    }
    if (n) printf("%d package%s to upgrade, %s to download (updates install)\n", n, n == 1 ? "" : "s", size_text(total));
    else if (checked && !err[0]) printf("Everything is up to date.\n");
    return n;
}

int main(int argc, char **argv) {
    const char *cmd = "status";
    int raw = 0, from_stdin = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--raw")) raw = 1;
        else if (!strcmp(argv[i], "-S")) from_stdin = 1;
        else if (!strcmp(argv[i], "-h") || !strcmp(argv[i], "--help")) {
            printf("usage: updates [status [--raw] | check | install [-S]]\n"
                   "The system's updates, through the installer service (man updates).\n");
            return 0;
        } else cmd = argv[i];
    }
    static char buf[65536];
    if (!strcmp(cmd, "status") || !strcmp(cmd, "check")) {
        if (!strcmp(cmd, "check") && !raw) printf("Checking...\n");
        if (ask(cmd) < 0 || reply(buf, sizeof buf) < 0) return 1;
        if (!strncmp(buf, "error", 5) && !strchr(buf, '\n')) {
            fprintf(stderr, "updates: %s\n", buf);
            return 1;
        }
        return show(buf, raw) ? 100 : 0;
    }
    if (strcmp(cmd, "install") && strcmp(cmd, "upgrade")) {
        fprintf(stderr, "updates: %s? (status, check, install)\n", cmd);
        return 2;
    }
    char pw[256] = "", req[300];
    if (geteuid() != 0) {
        if (from_stdin) {
            if (!fgets(pw, sizeof pw, stdin)) pw[0] = 0;
            pw[strcspn(pw, "\r\n")] = 0;
        } else {
            char *p = getpass("Your password: ");
            snprintf(pw, sizeof pw, "%s", p ? p : "");
            if (p) memset(p, 0, strlen(p));
        }
    }
    snprintf(req, sizeof req, "upgrade\n%s", pw);
    memset(pw, 0, sizeof pw);
    int r = ask(req);
    memset(req, 0, sizeof req);
    if (r < 0) return 1;
    for (;;) {
        if (reply(buf, sizeof buf) < 0) {
            fprintf(stderr, "updates: the installer service went away\n");
            return 1;
        }
        if (!strncmp(buf, "out ", 4)) {
            printf("%s\n", buf + 4);
            fflush(stdout);
        } else if (!strncmp(buf, "denied ", 7)) {
            fprintf(stderr, "updates: refused: %s\n", buf + 7);
            return 1;
        } else if (!strncmp(buf, "done ", 5)) {
            int code = atoi(buf + 5);
            if (code) fprintf(stderr, "updates: rpg upgrade ended with %d\n", code);
            return code ? 1 : 0;
        }
    }
}
