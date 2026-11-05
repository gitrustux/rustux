// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ls [-l] [-a] [PATH]...: list directory contents (a small stand-in until
// uutils' ls in step 11)

#include <dirent.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

static int long_format, all;

static int by_name(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

static void show(const char *dir, const char *name) {
    if (!long_format) {
        printf("%s\n", name);
        return;
    }
    char path[4096];
    struct stat st;
    snprintf(path, sizeof path, "%s/%s", dir, name);
    if (stat(dir[0] ? path : name, &st) < 0) {
        printf("?????????? %10s %s\n", "?", name);
        return;
    }
    char m[11] = "----------";
    if (S_ISDIR(st.st_mode)) m[0] = 'd';
    const char *rwx = "rwxrwxrwx";
    for (int i = 0; i < 9; i++) if (st.st_mode & (0400 >> i)) m[i + 1] = rwx[i];
    printf("%s %10lld %s%s\n", m, (long long)st.st_size, name, S_ISDIR(st.st_mode) ? "/" : "");
}

static int list(const char *path, int heading) {
    struct stat st;
    if (stat(path, &st) < 0) {
        fprintf(stderr, "ls: %s: %s\n", path, strerror(errno));
        return 1;
    }
    if (!S_ISDIR(st.st_mode)) {
        show("", path);
        return 0;
    }
    DIR *d = opendir(path);
    if (!d) {
        fprintf(stderr, "ls: %s: %s\n", path, strerror(errno));
        return 1;
    }
    char **names = 0;
    size_t n = 0, cap = 0;
    struct dirent *e;
    while ((e = readdir(d))) {
        if (e->d_name[0] == '.' && !all) continue;
        if (n == cap) {
            cap = cap ? cap * 2 : 64;
            names = realloc(names, cap * sizeof *names);
        }
        names[n++] = strdup(e->d_name);
    }
    closedir(d);
    qsort(names, n, sizeof *names, by_name);
    if (heading) printf("%s:\n", path);
    for (size_t i = 0; i < n; i++) {
        show(path, names[i]);
        free(names[i]);
    }
    free(names);
    return 0;
}

int main(int argc, char **argv) {
    int i = 1, status = 0;
    for (; i < argc && argv[i][0] == '-' && argv[i][1]; i++) {
        for (const char *o = argv[i] + 1; *o; o++) {
            if (*o == 'l') long_format = 1;
            else if (*o == 'a') all = 1;
            else { fprintf(stderr, "ls: unknown option -%c\n", *o); return 2; }
        }
    }
    if (i == argc) return list(".", 0);
    for (int k = i; k < argc; k++) {
        status |= list(argv[k], argc - i > 1);
        if (argc - i > 1 && k + 1 < argc) printf("\n");
    }
    return status;
}
