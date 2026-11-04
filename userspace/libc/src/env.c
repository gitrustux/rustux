// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Environment variables. `environ` is a malloc'd array; strings that came
// in the bootstrap message stay where they are, new ones are malloc'd.

#include "libc.h"
#include <errno.h>
#include <stdlib.h>
#include <string.h>

char **environ;
static size_t env_count, env_cap;

void __env_init(char **envp) {
    // envp is already a malloc'd NULL-terminated array we can own
    environ = envp;
    while (environ[env_count]) env_count++;
    env_cap = env_count + 1;
}

static int valid_name(const char *name) {
    return name && *name && !strchr(name, '=');
}

// Index of NAME in environ, or -1
static long find(const char *name, size_t len) {
    for (size_t i = 0; i < env_count; i++) {
        if (!strncmp(environ[i], name, len) && environ[i][len] == '=') return (long)i;
    }
    return -1;
}

char *getenv(const char *name) {
    if (!environ || !name) return 0;
    long i = find(name, strlen(name));
    return i < 0 ? 0 : environ[i] + strlen(name) + 1;
}

static int put(char *entry, size_t namelen) {
    long i = find(entry, namelen);
    if (i >= 0) {
        environ[i] = entry; // the old string may be in the bootstrap message: leaked if not
        return 0;
    }
    if (env_count + 2 > env_cap) {
        size_t cap = env_cap * 2 + 8;
        char **e = realloc(environ, cap * sizeof *e);
        if (!e) { errno = ENOMEM; return -1; }
        environ = e;
        env_cap = cap;
    }
    environ[env_count++] = entry;
    environ[env_count] = 0;
    return 0;
}

int setenv(const char *name, const char *value, int overwrite) {
    if (!valid_name(name)) { errno = EINVAL; return -1; }
    size_t n = strlen(name);
    if (find(name, n) >= 0 && !overwrite) return 0;
    size_t v = strlen(value);
    char *e = malloc(n + v + 2);
    if (!e) { errno = ENOMEM; return -1; }
    memcpy(e, name, n);
    e[n] = '=';
    memcpy(e + n + 1, value, v + 1);
    return put(e, n);
}

int putenv(char *string) {
    char *eq = strchr(string, '=');
    if (!eq || eq == string) { errno = EINVAL; return -1; }
    return put(string, (size_t)(eq - string));
}

int unsetenv(const char *name) {
    if (!valid_name(name)) { errno = EINVAL; return -1; }
    long i;
    while ((i = find(name, strlen(name))) >= 0) {
        memmove(&environ[i], &environ[i + 1], (env_count - (size_t)i) * sizeof *environ);
        env_count--;
    }
    return 0;
}

int clearenv(void) {
    env_count = 0;
    if (environ) environ[0] = 0;
    return 0;
}
