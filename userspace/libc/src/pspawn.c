// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// posix_spawn, waitpid and kill on top of rx_spawn. A process id is the
// process's koid. The library keeps a handle to each child it started,
// so waitpid can wait for it and read its exit code; kill finds other
// processes through our job.
//
// The child gets fds 0-2 (after the file actions are applied), a
// connection to our namespace and its current directory in $PWD; other
// fds are not passed on (as if all were close-on-exec).

#include "libc.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

// ---- file actions and attributes ----

enum { FA_DUP2, FA_CLOSE, FA_OPEN, FA_CHDIR };
struct action {
    int op, fd, newfd, flags;
    mode_t mode;
    char *path;
    struct action *next;
};

int posix_spawn_file_actions_init(posix_spawn_file_actions_t *fa) {
    memset(fa, 0, sizeof *fa);
    return 0;
}

int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *fa) {
    struct action *a = fa->__actions;
    while (a) {
        struct action *n = a->next;
        free(a->path);
        free(a);
        a = n;
    }
    fa->__actions = 0;
    return 0;
}

static int add(posix_spawn_file_actions_t *fa, int op, int fd, int newfd, const char *path, int flags, mode_t mode) {
    struct action *a = calloc(1, sizeof *a);
    if (!a || (path && !(a->path = strdup(path)))) {
        free(a);
        return ENOMEM;
    }
    a->op = op;
    a->fd = fd;
    a->newfd = newfd;
    a->flags = flags;
    a->mode = mode;
    struct action **p = (struct action **)&fa->__actions;
    while (*p) p = &(*p)->next;
    *p = a;
    return 0;
}

int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *fa, int fd, int newfd) {
    if (fd < 0 || newfd < 0 || newfd >= MAX_FDS) return EBADF;
    return add(fa, FA_DUP2, fd, newfd, 0, 0, 0);
}
int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *fa, int fd) {
    if (fd < 0 || fd >= MAX_FDS) return EBADF;
    return add(fa, FA_CLOSE, fd, 0, 0, 0, 0);
}
int posix_spawn_file_actions_addopen(posix_spawn_file_actions_t *restrict fa, int fd, const char *restrict path, int flags, mode_t mode) {
    if (fd < 0 || fd >= MAX_FDS) return EBADF;
    return add(fa, FA_OPEN, fd, 0, path, flags, mode);
}
int posix_spawn_file_actions_addchdir_np(posix_spawn_file_actions_t *restrict fa, const char *restrict path) {
    return add(fa, FA_CHDIR, 0, 0, path, 0, 0);
}

int posix_spawnattr_init(posix_spawnattr_t *a) { memset(a, 0, sizeof *a); return 0; }
int posix_spawnattr_destroy(posix_spawnattr_t *a) { (void)a; return 0; }
int posix_spawnattr_setflags(posix_spawnattr_t *a, short flags) { a->__flags = flags; return 0; }
int posix_spawnattr_getflags(const posix_spawnattr_t *restrict a, short *restrict flags) { *flags = (short)a->__flags; return 0; }
int posix_spawnattr_setsigmask(posix_spawnattr_t *restrict a, const sigset_t *restrict m) { a->__mask = *m; return 0; }
int posix_spawnattr_setsigdefault(posix_spawnattr_t *restrict a, const sigset_t *restrict d) { a->__def = *d; return 0; }
int posix_spawnattr_setpgroup(posix_spawnattr_t *a, pid_t g) { a->__pgrp = g; return 0; }

// ---- children ----

#define MAX_CHILDREN 64
static struct { pid_t pid; rx_handle_t proc; } children[MAX_CHILDREN];
static volatile int children_lock;

static uint64_t koid_of(rx_handle_t h) {
    rx_info_handle_basic_t b;
    return rx_object_get_info(h, RX_INFO_HANDLE_BASIC, &b, sizeof b, 0, 0) == RX_OK ? b.koid : 0;
}

static int remember(pid_t pid, rx_handle_t proc) {
    __lock(&children_lock);
    for (int i = 0; i < MAX_CHILDREN; i++) {
        if (!children[i].pid) {
            children[i].pid = pid;
            children[i].proc = proc;
            __unlock(&children_lock);
            return 0;
        }
    }
    __unlock(&children_lock);
    return -1;
}

// Our environment (or `envp`) with PWD set to `dir`
static char **with_pwd(char *const envp[], const char *dir) {
    size_t n = 0;
    while (envp && envp[n]) n++;
    char **e = malloc((n + 2) * sizeof *e);
    char *pwd = malloc(strlen(dir) + 5);
    if (!e || !pwd) {
        free(e);
        free(pwd);
        return 0;
    }
    strcpy(pwd, "PWD=");
    strcat(pwd, dir);
    size_t m = 0;
    for (size_t i = 0; i < n; i++)
        if (strncmp(envp[i], "PWD=", 4)) e[m++] = envp[i];
    e[m++] = pwd;
    e[m] = 0;
    return e;
}

static int spawn(pid_t *pid, const char *path, const posix_spawn_file_actions_t *fa, char *const argv[], char *const envp[]) {
    // The child's fds 0-2: which of our fds each one comes from (-1: none).
    // Files opened by the actions are opened here and closed afterwards.
    int src[3] = { 0, 1, 2 };
    int opened[8], nopened = 0;
    char dir[PATH_MAX];
    if (!getcwd(dir, sizeof dir)) strcpy(dir, "/");
    int err = 0;
    for (struct action *a = fa ? fa->__actions : 0; a && !err; a = a->next) {
        switch (a->op) {
        case FA_DUP2:
            if (a->newfd < 3) src[a->newfd] = a->fd < 3 ? src[a->fd] : a->fd;
            break;
        case FA_CLOSE:
            if (a->fd < 3) src[a->fd] = -1;
            break;
        case FA_OPEN: {
            if (a->fd >= 3 || nopened == 8) break;
            char p[PATH_MAX];
            if (a->path[0] != '/' && strlen(dir) + strlen(a->path) + 2 < sizeof p) {
                strcpy(p, dir);
                strcat(p, "/");
                strcat(p, a->path);
            } else {
                snprintf(p, sizeof p, "%s", a->path);
            }
            int fd = open(p, a->flags, a->mode);
            if (fd < 0) { err = errno; break; }
            opened[nopened++] = fd;
            src[a->fd] = fd;
            break;
        }
        case FA_CHDIR: {
            char p[PATH_MAX];
            if (a->path[0] == '/') snprintf(p, sizeof p, "%s", a->path);
            else snprintf(p, sizeof p, "%s/%s", dir, a->path);
            if (!realpath(p, dir)) err = errno;
            break;
        }
        }
    }
    rx_spawn_handle_t hs[3];
    uint32_t nh = 0;
    for (int i = 0; i < 3 && !err; i++) {
        if (src[i] < 0) continue;
        struct fd *f = src[i] < MAX_FDS ? &__fds[src[i]] : 0;
        if (!f || f->kind == RX_FD_NONE) { err = EBADF; break; }
        if (f->kind == RX_FD_LOG) continue;
        rx_handle_t h = __fd_export(src[i]);
        if (!h) { err = EBADF; break; }
        hs[nh].info = RX_FD_INFO(i, f->kind);
        hs[nh++].handle = h;
    }
    char **env = err ? 0 : with_pwd(envp ? envp : environ, dir);
    if (!err && !env) err = ENOMEM;
    for (int i = 0; i < nopened; i++) close(opened[i]);
    if (err) {
        for (uint32_t i = 0; i < nh; i++) rx_handle_close(hs[i].handle);
        return err;
    }
    rx_handle_t proc;
    rx_status_t r = rx_spawn(path, argv, env, RX_SPAWN_CLONE_NS, hs, nh, &proc);
    for (char **e = env; *e; e++)
        if (!strncmp(*e, "PWD=", 4) && !e[1]) free(*e); // the one we added (always last)
    free(env);
    if (r != RX_OK) return errno ? errno : ENOENT;
    pid_t p = (pid_t)koid_of(proc);
    if (remember(p, proc) < 0) {
        rx_handle_close(proc); // too many children to track: it runs, unwaited
    }
    if (pid) *pid = p;
    return 0;
}

int posix_spawn(pid_t *restrict pid, const char *restrict path, const posix_spawn_file_actions_t *fa,
                const posix_spawnattr_t *restrict attr, char *const argv[restrict], char *const envp[restrict]) {
    (void)attr; // signal masks and process groups mean nothing here
    return spawn(pid, path, fa, argv, envp);
}

int posix_spawnp(pid_t *restrict pid, const char *restrict file, const posix_spawn_file_actions_t *fa,
                 const posix_spawnattr_t *restrict attr, char *const argv[restrict], char *const envp[restrict]) {
    (void)attr;
    if (strchr(file, '/')) return spawn(pid, file, fa, argv, envp);
    const char *path = getenv("PATH");
    if (!path) path = "/bin";
    int err = ENOENT;
    while (*path) {
        size_t n = strcspn(path, ":");
        char p[PATH_MAX];
        if (n + strlen(file) + 2 < sizeof p) {
            memcpy(p, path, n);
            p[n] = 0;
            if (!n) strcpy(p, ".");
            strcat(p, "/");
            strcat(p, file);
            if (access(p, X_OK) == 0) return spawn(pid, p, fa, argv, envp);
            if (errno != ENOENT) err = errno;
        }
        path += n;
        if (*path == ':') path++;
    }
    return err;
}

// An exit status from a Rustux exit code
static int status_of(int64_t code) {
    if (code == RX_TASK_RETCODE_SYSCALL_KILL) return SIGKILL;
    if (code == -1025) return SIGSEGV; // crashed
    return (int)((code & 0xFF) << 8);
}

pid_t waitpid(pid_t pid, int *status, int options) {
    if (pid < -1 || pid == 0) pid = -1; // no process groups: any child
    __lock(&children_lock);
    rx_wait_item_t items[MAX_CHILDREN];
    int idx[MAX_CHILDREN], n = 0;
    for (int i = 0; i < MAX_CHILDREN; i++) {
        if (children[i].pid && (pid == -1 || children[i].pid == pid)) {
            items[n].handle = children[i].proc;
            items[n].waitfor = RX_PROCESS_TERMINATED;
            items[n].pending = 0;
            idx[n++] = i;
        }
    }
    __unlock(&children_lock);
    if (!n) return (pid_t)(errno = ECHILD, -1);
    rx_status_t r = rx_object_wait_many(items, (uint64_t)n, (options & WNOHANG) ? 0 : RX_TIME_INFINITE);
    if (r == RX_ERR_TIMED_OUT) return 0;
    if (r != RX_OK) return (pid_t)__set_errno_status(r);
    for (int k = 0; k < n; k++) {
        if (!(items[k].pending & RX_PROCESS_TERMINATED)) continue;
        __lock(&children_lock);
        int i = idx[k];
        pid_t p = children[i].pid;
        rx_handle_t h = children[i].proc;
        if (!p || h != items[k].handle) { // another thread took it
            __unlock(&children_lock);
            continue;
        }
        children[i].pid = 0;
        children[i].proc = 0;
        __unlock(&children_lock);
        rx_info_process_t info = { 0, 0, 0, 0 };
        rx_object_get_info(h, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
        rx_handle_close(h);
        if (status) *status = status_of(info.return_code);
        return p;
    }
    errno = ECHILD;
    return -1;
}

pid_t wait(int *status) { return waitpid(-1, status, 0); }

// No siginfo: Rust's std falls back to waitpid
int waitid(int idtype, unsigned id, void *info, int options) {
    (void)idtype; (void)id; (void)info; (void)options;
    errno = ENOSYS;
    return -1;
}

// Every signal but 0 (which only checks the process exists) ends the process
int kill(pid_t pid, int sig) {
    if (sig < 0 || sig >= NSIG) return (errno = EINVAL, -1);
    if (pid == getpid()) return raise(sig);
    rx_handle_t h = 0;
    int owned = 0;
    __lock(&children_lock);
    for (int i = 0; i < MAX_CHILDREN; i++)
        if (children[i].pid == pid && pid > 0) h = children[i].proc;
    __unlock(&children_lock);
    if (!h) {
        if (pid <= 0 || rx_object_get_child(rx_job_default, (uint64_t)pid, RX_RIGHT_SAME_RIGHTS, &h) != RX_OK)
            return (errno = ESRCH, -1);
        owned = 1;
    }
    rx_status_t r = sig ? rx_task_kill(h) : RX_OK;
    if (owned) rx_handle_close(h);
    return r == RX_OK ? 0 : __set_errno_status(r);
}
