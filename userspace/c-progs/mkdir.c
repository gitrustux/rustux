// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mkdir [-p] DIR...: make directories (-p: with their parents, no error if
// they exist)

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>

static int make(char *path, int parents) {
    if (!parents) return mkdir(path, 0755);
    for (char *p = path + 1; *p; p++) {
        if (*p != '/') continue;
        *p = 0;
        int r = mkdir(path, 0755);
        *p = '/';
        if (r < 0 && errno != EEXIST) return -1;
    }
    if (mkdir(path, 0755) < 0 && errno != EEXIST) return -1;
    return 0;
}

int main(int argc, char **argv) {
    int parents = 0, status = 0, i = 1;
    if (i < argc && !strcmp(argv[i], "-p")) { parents = 1; i++; }
    if (i == argc) { fprintf(stderr, "usage: mkdir [-p] DIR...\n"); return 2; }
    for (; i < argc; i++) {
        if (make(argv[i], parents) < 0) {
            fprintf(stderr, "mkdir: %s: %s\n", argv[i], strerror(errno));
            status = 1;
        }
    }
    return status;
}
