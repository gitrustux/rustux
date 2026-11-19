// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// portaltest: an app with no files: permission that uses the file chooser
// (FoxDen step FD6, <rustux/portal.h>; scripts/run-appupdates-test.sh
// makes an app of it). It checks:
//   - ~/Documents/note.txt cannot be opened by its path (no permission);
//   - rx_portal_open: the user picks note.txt, it reads it;
//   - the descriptor is that file alone: no folder to open beside it;
//   - the path still does not open afterwards;
//   - rx_portal_save: the user saves copy.txt, it writes it;
//   - a third question the user says no to: -ECANCELED.
// Each check prints "portaltest: NAME ok|FAIL"; then "portaltest: passed
// N of M".

#include <errno.h>
#include <fcntl.h>
#include <rustux/portal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int passed, total;

static void check(const char *name, int ok, const char *why) {
    total++;
    passed += ok;
    printf("portaltest: %s %s%s%s%s\n", name, ok ? "ok" : "FAIL", ok ? "" : " (", ok ? "" : why, ok ? "" : ")");
    fflush(stdout);
}

int main(void) {
    const char *home = getenv("HOME");
    if (!home) home = "/";
    char path[512], name[256], buf[256], why[300];
    snprintf(path, sizeof path, "%s/Documents/note.txt", home);

    int fd = open(path, O_RDONLY);
    check("path-refused", fd < 0, "opened ~/Documents/note.txt by its path");
    if (fd >= 0) close(fd);

    fd = rx_portal_open("Open a note to read", "*.txt", name, sizeof name);
    snprintf(why, sizeof why, "rx_portal_open: %s", fd < 0 ? strerror(-fd) : "");
    check("chooser-open", fd >= 0 && !strcmp(name, "note.txt"), why);
    if (fd >= 0) {
        ssize_t n = read(fd, buf, sizeof buf - 1);
        buf[n > 0 ? n : 0] = 0;
        buf[strcspn(buf, "\n")] = 0;
        printf("portaltest: read %zd bytes from %s: %s\n", n, name, buf);
        check("read", n > 0 && strstr(buf, "a note for portaltest"), buf);
        struct stat st;
        check("fstat", fstat(fd, &st) == 0 && S_ISREG(st.st_mode), "fstat");
        int other = openat(fd, "other.txt", O_RDONLY);
        check("no-folder", other < 0, "opened a file beside it");
        if (other >= 0) close(other);
        char w[] = "changed";
        check("read-only", write(fd, w, sizeof w) < 0, "wrote to a file opened for reading");
        close(fd);
    }
    fd = open(path, O_RDONLY);
    check("path-still-refused", fd < 0, "the path opened after the chooser");
    if (fd >= 0) close(fd);

    fd = rx_portal_save("Save a copy", "copy.txt", name, sizeof name);
    snprintf(why, sizeof why, "rx_portal_save: %s", fd < 0 ? strerror(-fd) : "");
    check("chooser-save", fd >= 0 && !strcmp(name, "copy.txt"), why);
    if (fd >= 0) {
        const char *t = "a copy saved by portaltest\n";
        check("write", write(fd, t, strlen(t)) == (ssize_t)strlen(t), "write");
        close(fd);
    }

    fd = rx_portal_open("Pick nothing (say Cancel)", 0, name, sizeof name);
    check("cancel", fd == -ECANCELED, fd >= 0 ? "a file was given" : strerror(-fd));
    if (fd >= 0) close(fd);

    printf("portaltest: passed %d of %d\n", passed, total);
    fflush(stdout);
    return passed == total ? 0 : 1;
}
