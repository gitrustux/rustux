// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ptytest: pseudo-terminals (DESKTOP.md step F7). Runs itself and then the
// shell on a pseudo-terminal and checks the size, the line discipline
// (echo, Backspace, cooked and raw mode, Ctrl+C) and the end (EIO).
//
//   ptytest          the test (prints "ptytest: ..." lines, then "ptytest: ok")
//   ptytest --child  the program on the slave side

#include <errno.h>
#include <fcntl.h>
#include <rustux/process.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static int child(void) {
    struct winsize w;
    if (ioctl(0, TIOCGWINSZ, &w) < 0) printf("no size\n");
    else printf("size %ux%u\n", w.ws_col, w.ws_row);
    fflush(stdout);
    char line[128];
    if (!fgets(line, sizeof line, stdin)) return 1;
    line[strcspn(line, "\n")] = 0;
    printf("got [%s]\n", line);
    fflush(stdout);
    struct termios t;
    tcgetattr(0, &t);
    cfmakeraw(&t);
    tcsetattr(0, TCSANOW, &t);
    char c;
    if (read(0, &c, 1) != 1) return 1;
    printf("raw [%c]\n", c);
    fflush(stdout);
    return 0;
}

static int master;
static char seen[16384];
static size_t nseen, mark; // what was read; where the next expect looks from

// Read the master until `want` has appeared after the last one found (or
// 10 s); 0 if it did
static int expect(const char *want) {
    uint64_t end = (uint64_t)time(0) + 10;
    int fl = fcntl(master, F_GETFL);
    fcntl(master, F_SETFL, fl | O_NONBLOCK);
    for (;;) {
        seen[nseen] = 0;
        char *at = strstr(seen + mark, want);
        if (at) {
            mark = (size_t)(at - seen) + strlen(want);
            fcntl(master, F_SETFL, fl);
            return 0;
        }
        if ((uint64_t)time(0) > end) break;
        ssize_t n = read(master, seen + nseen, sizeof seen - 1 - nseen);
        if (n > 0) nseen += (size_t)n;
        else if (n < 0 && errno == EIO) break;
        else usleep(20000);
    }
    fcntl(master, F_SETFL, fl);
    printf("ptytest: FAIL: no \"%s\" in:\n%s\n", want, seen);
    return -1;
}

static pid_t spawn_on(const char *path, char *const argv[]) {
    int slave = open(ptsname(master), O_RDWR);
    if (slave < 0) {
        printf("ptytest: FAIL: open %s: %s\n", ptsname(master), strerror(errno));
        return -1;
    }
    posix_spawn_file_actions_t fa;
    posix_spawn_file_actions_init(&fa);
    for (int i = 0; i < 3; i++) posix_spawn_file_actions_adddup2(&fa, slave, i);
    pid_t pid;
    int r = posix_spawn(&pid, path, &fa, 0, argv, environ);
    close(slave);
    if (r) {
        printf("ptytest: FAIL: spawn %s: %s\n", path, strerror(r));
        return -1;
    }
    return pid;
}

static int eof(void) {
    char buf[256];
    uint64_t end = (uint64_t)time(0) + 5;
    int fl = fcntl(master, F_GETFL);
    fcntl(master, F_SETFL, fl | O_NONBLOCK);
    while ((uint64_t)time(0) <= end) {
        ssize_t n = read(master, buf, sizeof buf);
        if (n < 0 && errno == EIO) return 0;
        if (n <= 0) usleep(20000);
    }
    printf("ptytest: FAIL: the master did not see the end\n");
    return -1;
}

static int open_master(void) {
    master = posix_openpt(O_RDWR | O_NOCTTY);
    if (master < 0 || grantpt(master) || unlockpt(master) || !ptsname(master)) {
        printf("ptytest: FAIL: posix_openpt\n");
        return -1;
    }
    nseen = mark = 0;
    return 0;
}

int main(int argc, char **argv) {
    if (argc > 1 && !strcmp(argv[1], "--child")) return child();

    // 1. A program of our own: size, cooked mode with echo, raw mode
    if (open_master() < 0) return 1;
    struct winsize w = { 30, 100, 0, 0 };
    if (ioctl(master, TIOCSWINSZ, &w) < 0) printf("ptytest: FAIL: TIOCSWINSZ\n");
    if (!isatty(master)) printf("ptytest: FAIL: isatty(master)\n");
    char *cargv[] = { "/bin/ptytest", "--child", 0 };
    if (spawn_on("/bin/ptytest", cargv) < 0) return 1;
    if (expect("size 100x30")) return 1;
    printf("ptytest: size 100x30\n");
    write(master, "abc\177d\r", 6); // Backspace, Enter as CR
    if (expect("abc\b \bd\n") || expect("got [abd]")) return 1;
    printf("ptytest: cooked, echo, backspace\n");
    write(master, "x", 1);
    if (expect("raw [x]")) return 1;
    printf("ptytest: raw\n");
    if (eof()) return 1;
    printf("ptytest: end of the program, EIO\n");
    if (open(ptsname(master), O_RDWR) >= 0) printf("ptytest: FAIL: the slave opened twice\n");
    close(master);

    // 2. The shell on a pseudo-terminal: a command, Ctrl+C, exit
    if (open_master() < 0) return 1;
    char *sargv[] = { "/bin/shell", 0 };
    if (spawn_on("/bin/shell", sargv) < 0) return 1;
    // Each line is typed at a prompt (typing ahead works, but Ctrl+C
    // throws away what was typed ahead)
    const char *prompt = "\033[0m > ";
    if (expect(prompt)) return 1;
    write(master, "echo pty-$((6*7))\r", 18);
    if (expect("pty-42") || expect(prompt)) return 1;
    printf("ptytest: shell command\n");
    write(master, "sleep 30\r", 9);
    if (expect("sleep 30")) return 1;
    usleep(1000000);
    write(master, "\003", 1);
    if (expect(prompt)) return 1; // sleep ended, long before 30 s
    write(master, "echo af''ter\r", 13);
    if (expect("after\n")) return 1;
    printf("ptytest: Ctrl+C\n");
    write(master, "exit\r", 5);
    if (eof()) return 1;
    printf("ptytest: shell exit, EIO\n");
    close(master);
    printf("ptytest: ok\n");
    return 0;
}
