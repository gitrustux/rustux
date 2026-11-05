// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// cat [FILE]...: copy each FILE (or standard input for "-" or no FILE) to
// standard output

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static int copy(int fd, const char *name) {
    char buf[16384];
    for (;;) {
        ssize_t n = read(fd, buf, sizeof buf);
        if (n == 0) return 0;
        if (n < 0) {
            fprintf(stderr, "cat: %s: %s\n", name, strerror(errno));
            return 1;
        }
        for (ssize_t done = 0; done < n;) {
            ssize_t w = write(1, buf + done, (size_t)(n - done));
            if (w <= 0) return 1; // stdout gone (e.g. the reader of a pipe ended)
            done += w;
        }
    }
}

int main(int argc, char **argv) {
    int status = 0;
    if (argc < 2) return copy(0, "stdin");
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-")) {
            status |= copy(0, "stdin");
            continue;
        }
        int fd = open(argv[i], O_RDONLY);
        if (fd < 0) {
            fprintf(stderr, "cat: %s: %s\n", argv[i], strerror(errno));
            status = 1;
            continue;
        }
        status |= copy(fd, argv[i]);
        close(fd);
    }
    return status;
}
