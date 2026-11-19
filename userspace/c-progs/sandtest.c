// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// sandtest: an app that tries to leave its sandbox (FoxDen step FD2;
// scripts/run-sandbox-test.sh builds two apps around it)
//
//   sandtest [net] [docs] [port=N] [color=RRGGBB] [at=X,Y] [hold=SECONDS]
//
// net and docs say which permissions it was given (and so what must work);
// everything else must be refused. Each check prints "sandtest: NAME ok"
// or "... FAIL (why)"; the summary ("sandtest: APP_ID passed N of M")
// goes to the standard output and to $XDG_DATA_HOME/result. Then it shows
// a window of `color` for `hold` seconds.

#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/syscalls.h>
#include <rustux/window.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

static int passed, total;
static char summary[4096];
static size_t slen;

static void check(const char *name, int ok, const char *why) {
    total++;
    passed += ok;
    printf("sandtest: %s %s%s%s%s\n", name, ok ? "ok" : "FAIL", ok ? "" : " (", ok ? "" : why, ok ? "" : ")");
    if (slen < sizeof summary - 200)
        slen += (size_t)snprintf(summary + slen, sizeof summary - slen, "%s %s\n", name, ok ? "ok" : "FAIL");
}

static int can_read(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return 0;
    char b[16];
    ssize_t n = read(fd, b, sizeof b);
    close(fd);
    return n >= 0;
}

static int can_write(const char *path) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (fd < 0) return 0;
    ssize_t n = write(fd, "x\n", 2);
    close(fd);
    return n == 2;
}

static int entries(const char *path, char *names, size_t cap) {
    DIR *d = opendir(path);
    if (!d) return -1;
    int n = 0;
    size_t at = 0;
    struct dirent *e;
    names[0] = 0;
    while ((e = readdir(d))) {
        if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, "..")) continue;
        n++;
        at += (size_t)snprintf(names + at, at < cap ? cap - at : 0, "%s%s", at ? " " : "", e->d_name);
        if (at >= cap) at = cap - 1;
    }
    closedir(d);
    return n;
}

static int net_works(int port) {
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) return 0;
    struct sockaddr_in a = { 0 };
    a.sin_family = AF_INET;
    a.sin_port = htons((uint16_t)port);
    a.sin_addr.s_addr = inet_addr("10.0.2.2");
    int ok = connect(s, (struct sockaddr *)&a, sizeof a) == 0;
    if (ok) {
        char b[64] = { 0 };
        ssize_t n = read(s, b, sizeof b - 1);
        ok = n > 0 && !strncmp(b, "hello", 5);
    }
    close(s);
    return ok;
}

int main(int argc, char **argv) {
    int net = 0, docs = 0, port = 18765, hold = 0, wx = -1, wy = -1;
    uint32_t color = 0x00a0ff;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "net")) net = 1;
        else if (!strcmp(argv[i], "docs")) docs = 1;
        else if (!strncmp(argv[i], "port=", 5)) port = atoi(argv[i] + 5);
        else if (!strncmp(argv[i], "hold=", 5)) hold = atoi(argv[i] + 5);
        else if (!strncmp(argv[i], "color=", 6)) color = (uint32_t)strtoul(argv[i] + 6, 0, 16);
        else if (!strncmp(argv[i], "at=", 3)) {
            char *e;
            wx = (int)strtol(argv[i] + 3, &e, 10);
            wy = *e == ',' ? atoi(e + 1) : -1;
        }
    }
    const char *id = getenv("APP_ID"), *home = getenv("HOME"), *data = getenv("XDG_DATA_HOME");
    if (!id || !home || !data) {
        printf("sandtest: not started by appctl (no APP_ID, HOME or XDG_DATA_HOME)\n");
        return 1;
    }
    char p[600], q[600], names[1024];

    // What it may see
    snprintf(p, sizeof p, "/apps/%s/manifest", id);
    check("own-manifest", can_read(p), "its own manifest is not readable");
    snprintf(p, sizeof p, "%s/note", data);
    check("data-write", can_write(p) && can_read(p), "its data folder is not writable");
    check("dev-null", can_write("/dev/null"), "/dev/null");
    check("usr", can_read("/usr/share/fonts/inter/Inter-Regular.ttf"), "/usr is not readable");

    // What it may not
    check("etc-shadow", !can_read("/etc/shadow"), "/etc/shadow was readable");
    check("bin", !can_read("/bin/shell"), "/bin/shell was readable");
    int n = entries("/apps", names, sizeof names);
    check("apps-own-only", n == 1 && !strcmp(names, id), names);
    snprintf(p, sizeof p, "/apps/%s/planted", id);
    check("apps-readonly", !can_write(p), "wrote into its own folder");
    snprintf(p, sizeof p, "%s/secret", home);
    check("home-hidden", !can_read(p), "~/secret was readable");
    n = entries(home, names, sizeof names);
    check("home-list", n >= 0 && !strstr(names, "secret"), names);
    check("etc-list", entries("/etc", names, sizeof names) >= 0 && !strstr(names, "shadow") && !strstr(names, "rpg"), names);
    check("root-list", entries("/", names, sizeof names) >= 0 && !strstr(names, "bin") && !strstr(names, "var"), names);
    snprintf(p, sizeof p, "%s/escape", data);
    unlink(p);
    symlink("/etc/shadow", p);
    check("link-escape", !can_read(p), "a link to /etc/shadow was followed");
    snprintf(p, sizeof p, "%s/up", data);
    unlink(p);
    symlink("../../../../../../..", p);
    snprintf(q, sizeof q, "%s/up%s/secret", data, home);
    check("link-up", !can_read(q), "a link up and back down reached ~/secret");
    snprintf(p, sizeof p, "%s/../..", data);
    check("dotdot", entries(p, names, sizeof names) < 0 || !strstr(names, "secret"), "..");
    rx_handle_t h = 0;
    int r = rx_service_connect("/svc/auth", &h);
    check("svc-auth", r < 0, "connected to /svc/auth");
    if (r == 0) rx_handle_close(h);
    r = rx_service_connect("/svc/packages", &h);
    check("svc-packages", r < 0, "connected to /svc/packages");
    if (r == 0) rx_handle_close(h);
    rx_handle_t proc = 0;
    char *sh[] = { "/bin/shell", "-c", "true", 0 };
    rx_status_t s = rx_spawn("/bin/shell", sh, 0, 0, 0, 0, &proc);
    check("spawn-shell", s != RX_OK, "started /bin/shell");
    rx_creds_t creds;
    memset(&creds, 0, sizeof creds);
    rx_process_get_creds(rx_proc_self, &creds);
    s = rx_process_set_creds(&creds);
    check("set-creds", s == RX_ERR_ACCESS_DENIED, "set_creds was not refused by the policy");
    s = RX_SYS(0x50, "x", 1, 0, 0);
    check("debug-write", s == RX_ERR_ACCESS_DENIED, "the kernel console was writable");

    // What its permissions give
    check(net ? "network" : "no-network", net_works(port) == net, net ? "could not reach 10.0.2.2" : "reached 10.0.2.2");
    snprintf(p, sizeof p, "%s/Documents/note.txt", home);
    check(docs ? "documents" : "no-documents", can_read(p) == docs, docs ? "~/Documents not readable" : "~/Documents readable");
    if (docs) {
        snprintf(p, sizeof p, "%s/Documents/from-%s.txt", home, id);
        check("documents-write", can_write(p), "~/Documents not writable");
    }

    printf("sandtest: %s passed %d of %d\n", id, passed, total);
    fflush(stdout);
    snprintf(p, sizeof p, "%s/result", data);
    FILE *f = fopen(p, "w");
    if (f) {
        fprintf(f, "%s%s passed %d of %d\n", summary, id, passed, total);
        fclose(f);
    }

    if (hold > 0) {
        rx_window_t win;
        if (rx_window_create(&win, wx, wy, 240, 120, id) == 0) {
            for (uint64_t i = 0; i < (uint64_t)win.width * win.height; i++) win.pixels[i] = 0xff000000u | color;
            rx_window_commit(&win, 0, 0);
            printf("sandtest: window shown\n");
            fflush(stdout);
            uint64_t end = rx_clock_get_monotonic() + (uint64_t)hold * 1000000000ull;
            rx_window_event_t ev;
            while (rx_clock_get_monotonic() < end) {
                if (rx_window_next_event(&win, &ev, end) == 0 && ev.op == RX_WINDOW_EV_CLOSE_REQUEST) break;
            }
            rx_window_destroy(&win);
        } else {
            printf("sandtest: no window\n");
        }
    }
    return passed == total ? 0 : 1;
}
