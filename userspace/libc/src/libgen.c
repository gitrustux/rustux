// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// basename and dirname, as POSIX says (and glibc's libgen.h, musl):
// trailing slashes are dropped; "" and NULL give "."
#include <libgen.h>
#include <string.h>

char *basename(char *path) {
    if (!path || !*path) return ".";
    size_t n = strlen(path);
    while (n > 1 && path[n - 1] == '/') path[--n] = 0;
    char *s = strrchr(path, '/');
    return s && s[1] ? s + 1 : path;
}

char *dirname(char *path) {
    if (!path || !*path) return ".";
    size_t n = strlen(path);
    while (n > 1 && path[n - 1] == '/') n--;
    while (n > 0 && path[n - 1] != '/') n--;
    if (n == 0) return ".";
    while (n > 1 && path[n - 1] == '/') n--;
    path[n] = 0;
    return path;
}
