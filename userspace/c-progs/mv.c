// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mv SOURCE DEST, or mv SOURCE... DIR: rename or move files

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: mv SOURCE DEST | mv SOURCE... DIR\n"); return 2; }
    const char *dest = argv[argc - 1];
    struct stat st;
    int to_dir = stat(dest, &st) == 0 && S_ISDIR(st.st_mode);
    if (argc > 3 && !to_dir) { fprintf(stderr, "mv: %s: not a directory\n", dest); return 1; }
    int status = 0;
    for (int i = 1; i < argc - 1; i++) {
        char target[4096];
        const char *base = strrchr(argv[i], '/') ? strrchr(argv[i], '/') + 1 : argv[i];
        if (to_dir) snprintf(target, sizeof target, "%s/%s", dest, base);
        else snprintf(target, sizeof target, "%s", dest);
        if (rename(argv[i], target) < 0) {
            fprintf(stderr, "mv: %s -> %s: %s\n", argv[i], target, strerror(errno));
            status = 1;
        }
    }
    return status;
}
