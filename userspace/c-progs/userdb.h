// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// userdb.h: what login, authd, sudo/su/passwd and useradd share about
// users (step 17): /etc/passwd, /etc/group and /etc/shadow as lines of
// ':'-separated fields, changed by rewriting the file (a new file renamed
// over the old one), password checks, and the credentials of a user.

#ifndef USERDB_H
#define USERDB_H

#include <crypt.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <pwd.h>
#include <rustux/syscalls.h>
#include <shadow.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define UDB_PASSWD "/etc/passwd"
#define UDB_GROUP  "/etc/group"
#define UDB_SHADOW "/etc/shadow"

// A file's lines (malloc'd, without their '\n'); NULL-terminated
static __attribute__((unused)) char **udb_read(const char *path, int *count) {
    FILE *f = fopen(path, "r");
    int n = 0, cap = 16;
    char **v = malloc(sizeof *v * (size_t)cap), line[2048];
    if (!v) return 0;
    while (f && fgets(line, sizeof line, f)) {
        line[strcspn(line, "\n")] = 0;
        if (n + 2 > cap) {
            cap *= 2;
            char **nv = realloc(v, sizeof *v * (size_t)cap);
            if (!nv) break;
            v = nv;
        }
        v[n++] = strdup(line);
    }
    if (f) fclose(f);
    v[n] = 0;
    if (count) *count = n;
    return v;
}

static __attribute__((unused)) void udb_free(char **v) {
    for (int i = 0; v && v[i]; i++) free(v[i]);
    free(v);
}

// Write the lines to `path` (through PATH+ renamed over it), mode `mode`
static __attribute__((unused)) int udb_write(const char *path, char **v, mode_t mode) {
    char tmp[256];
    snprintf(tmp, sizeof tmp, "%s+", path);
    int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, mode);
    if (fd < 0) return -errno;
    FILE *f = fdopen(fd, "w");
    if (!f) { close(fd); return -ENOMEM; }
    for (int i = 0; v[i]; i++) fprintf(f, "%s\n", v[i]);
    int bad = ferror(f);
    if (fclose(f) != 0 || bad) { unlink(tmp); return -EIO; }
    chmod(tmp, mode);
    if (rename(tmp, path) < 0) { int e = errno; unlink(tmp); return -e; }
    return 0;
}

// The line whose first field is `name` (index), or -1
static __attribute__((unused)) int udb_find(char **v, const char *name) {
    size_t n = strlen(name);
    for (int i = 0; v && v[i]; i++)
        if (!strncmp(v[i], name, n) && v[i][n] == ':') return i;
    return -1;
}

// Field k (0-based) of a line, copied into out
static __attribute__((unused)) const char *udb_field(const char *line, int k, char *out, size_t cap) {
    const char *p = line;
    for (int i = 0; i < k && p; i++) {
        p = strchr(p, ':');
        if (p) p++;
    }
    if (!p) { out[0] = 0; return out; }
    size_t n = strcspn(p, ":");
    if (n >= cap) n = cap - 1;
    memcpy(out, p, n);
    out[n] = 0;
    return out;
}

// The line with field k replaced (a new malloc'd string)
static __attribute__((unused)) char *udb_set_field(const char *line, int k, const char *val) {
    const char *p = line;
    for (int i = 0; i < k && p; i++) {
        p = strchr(p, ':');
        if (p) p++;
    }
    if (!p) return strdup(line);
    const char *end = p + strcspn(p, ":");
    size_t a = (size_t)(p - line), b = strlen(end);
    char *r = malloc(a + strlen(val) + b + 1);
    if (!r) return 0;
    memcpy(r, line, a);
    strcpy(r + a, val);
    strcat(r, end);
    return r;
}

static __attribute__((unused)) int udb_valid_name(const char *s) {
    if (!*s || strlen(s) > 32 || *s == '-' || (*s >= '0' && *s <= '9')) return 0;
    for (; *s; s++)
        if (!((*s >= 'a' && *s <= 'z') || (*s >= '0' && *s <= '9') || *s == '_' || *s == '-' || *s == '.')) return 0;
    return 1;
}

// A user's password hash: from /etc/shadow, else /etc/passwd's field;
// "" if the user needs none, "!" if it cannot log in with one, 0: no such
// user
static __attribute__((unused)) const char *udb_hash(const char *user) {
    static char h[256];
    struct passwd *pw = getpwnam(user);
    if (!pw) return 0;
    struct spwd *sp = getspnam(user);
    const char *src = sp ? sp->sp_pwdp : strcmp(pw->pw_passwd, "x") ? pw->pw_passwd : "!";
    snprintf(h, sizeof h, "%s", src ? src : "!");
    return h;
}

// Is `pw` the user's password? (an empty hash takes any, even none)
static __attribute__((unused)) int udb_check(const char *user, const char *pw) {
    const char *h = udb_hash(user);
    if (!h) {
        crypt(pw ? pw : "", "$6$aaaaaaaaaaaaaaaa$");   // as slow as a real check
        return 0;
    }
    if (!*h) return 1;
    if (h[0] == '!' || h[0] == '*' || strncmp(h, "$6$", 3)) return 0;
    const char *c = crypt(pw ? pw : "", h);
    return c && !strcmp(c, h);
}

static __attribute__((unused)) int udb_locked(const char *user) {
    const char *h = udb_hash(user);
    return !h || h[0] == '!' || h[0] == '*';
}

// Set (or with pw NULL: lock) a user's password in /etc/shadow
static __attribute__((unused)) int udb_set_password(const char *user, const char *pw) {
    char hash[256];
    if (pw) {
        const char *s = crypt_gensalt("$6$", 0, 0, 0), *c = s ? crypt(pw, s) : 0;
        if (!c) return -EIO;
        snprintf(hash, sizeof hash, "%s", c);
    } else {
        strcpy(hash, "!");
    }
    int n;
    char **v = udb_read(UDB_SHADOW, &n);
    if (!v) return -ENOMEM;
    char day[24];
    snprintf(day, sizeof day, "%ld", (long)(time(0) / 86400));
    int i = udb_find(v, user);
    if (i < 0) {
        char line[512];
        snprintf(line, sizeof line, "%s:%s:%s:0:99999:7:::", user, hash, day);
        char **nv = realloc(v, sizeof *v * (size_t)(n + 2));
        if (!nv) { udb_free(v); return -ENOMEM; }
        v = nv;
        v[n] = strdup(line);
        v[n + 1] = 0;
    } else {
        char *a = udb_set_field(v[i], 1, hash), *b = a ? udb_set_field(a, 2, day) : 0;
        free(a);
        if (!b) { udb_free(v); return -ENOMEM; }
        free(v[i]);
        v[i] = b;
    }
    int r = udb_write(UDB_SHADOW, v, 0600);
    udb_free(v);
    return r;
}

// What a process acting as `pw` has: its ids, its groups from /etc/group
static __attribute__((unused)) void udb_creds(const struct passwd *pw, rx_creds_t *c) {
    memset(c, 0, sizeof *c);
    c->ruid = c->euid = c->suid = pw->pw_uid;
    c->rgid = c->egid = c->sgid = pw->pw_gid;
    c->umask = 022;
    gid_t g[RX_MAX_GROUPS];
    int n = RX_MAX_GROUPS;
    if (getgrouplist(pw->pw_name, pw->pw_gid, g, &n) < 0) n = RX_MAX_GROUPS;
    for (int i = 0; i < n && i < RX_MAX_GROUPS; i++) c->groups[i] = g[i];
    c->ngroups = (uint32_t)(n < RX_MAX_GROUPS ? n : RX_MAX_GROUPS);
}

// The environment of a login as `pw` (static strings, NULL-terminated)
static __attribute__((unused)) char **udb_login_env(const struct passwd *pw, const char *keep_term) {
    static char home[300], user[64], logname[80], shell[300], pwd[300], term[64];
    static char *env[10];
    int n = 0;
    snprintf(home, sizeof home, "HOME=%s", pw->pw_dir);
    snprintf(user, sizeof user, "USER=%s", pw->pw_name);
    snprintf(logname, sizeof logname, "LOGNAME=%s", pw->pw_name);
    snprintf(shell, sizeof shell, "SHELL=%s", *pw->pw_shell ? pw->pw_shell : "/bin/shell");
    struct stat st;
    snprintf(pwd, sizeof pwd, "PWD=%s", stat(pw->pw_dir, &st) == 0 && S_ISDIR(st.st_mode) ? pw->pw_dir : "/");
    env[n++] = "PATH=/bin";
    env[n++] = home;
    env[n++] = user;
    env[n++] = logname;
    env[n++] = shell;
    env[n++] = pwd;
    if (keep_term) {
        snprintf(term, sizeof term, "TERM=%s", keep_term);
        env[n++] = term;
    }
    env[n] = 0;
    return env;
}

#endif
