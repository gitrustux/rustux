// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// rm [-r] [-f] PATH...: remove files (-r: directories and their contents;
// -f: no error for missing files)

#include <dirent.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int recursive, force;

static int remove_tree(const char *path) {
    struct stat st;
    if (stat(path, &st) < 0) {
        if (force && errno == ENOENT) return 0;
        fprintf(stderr, "rm: %s: %s\n", path, strerror(errno));
        return 1;
    }
    if (!S_ISDIR(st.st_mode)) {
        if (unlink(path) == 0) return 0;
        fprintf(stderr, "rm: %s: %s\n", path, strerror(errno));
        return 1;
    }
    if (!recursive) {
        fprintf(stderr, "rm: %s: is a directory (use -r)\n", path);
        return 1;
    }
    int status = 0;
    for (;;) {
        // Re-open each round: entries are removed as we go
        DIR *d = opendir(path);
        if (!d) { fprintf(stderr, "rm: %s: %s\n", path, strerror(errno)); return 1; }
        struct dirent *e = readdir(d);
        char child[4096];
        if (e) snprintf(child, sizeof child, "%s/%s", path, e->d_name);
        closedir(d);
        if (!e) break;
        if (remove_tree(child)) { status = 1; break; }
    }
    if (status == 0 && rmdir(path) < 0) {
        fprintf(stderr, "rm: %s: %s\n", path, strerror(errno));
        status = 1;
    }
    return status;
}

int main(int argc, char **argv) {
    int i = 1, status = 0;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        for (const char *o = argv[i] + 1; *o; o++) {
            if (*o == 'r' || *o == 'R') recursive = 1;
            else if (*o == 'f') force = 1;
            else { fprintf(stderr, "rm: unknown option -%c\n", *o); return 2; }
        }
    }
    if (i == argc && !force) { fprintf(stderr, "usage: rm [-r] [-f] PATH...\n"); return 2; }
    for (; i < argc; i++) status |= remove_tree(argv[i]);
    return status;
}
