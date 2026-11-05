// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// posix_spawn: start a program (Rustux has no fork/exec). File actions
// (dup2, close, open, chdir) shape the child's fds 0-2 and its directory.

#ifndef _SPAWN_H
#define _SPAWN_H

#include <signal.h>
#include <sys/types.h>

typedef struct { int __pad0[2]; void *__actions; int __pad[16]; } posix_spawn_file_actions_t;
typedef struct {
    int __flags;
    pid_t __pgrp;
    sigset_t __def, __mask;
    int __prio, __pol;
    void *__fn;
    char __pad[64 - sizeof(void *)];
} posix_spawnattr_t;

#define POSIX_SPAWN_RESETIDS 1
#define POSIX_SPAWN_SETPGROUP 2
#define POSIX_SPAWN_SETSIGDEF 4
#define POSIX_SPAWN_SETSIGMASK 8
#define POSIX_SPAWN_SETSID 128

int posix_spawn(pid_t *restrict pid, const char *restrict path, const posix_spawn_file_actions_t *fa,
                const posix_spawnattr_t *restrict attr, char *const argv[restrict], char *const envp[restrict]);
int posix_spawnp(pid_t *restrict pid, const char *restrict file, const posix_spawn_file_actions_t *fa,
                 const posix_spawnattr_t *restrict attr, char *const argv[restrict], char *const envp[restrict]);
int posix_spawn_file_actions_init(posix_spawn_file_actions_t *fa);
int posix_spawn_file_actions_destroy(posix_spawn_file_actions_t *fa);
int posix_spawn_file_actions_adddup2(posix_spawn_file_actions_t *fa, int fd, int newfd);
int posix_spawn_file_actions_addclose(posix_spawn_file_actions_t *fa, int fd);
int posix_spawn_file_actions_addopen(posix_spawn_file_actions_t *restrict fa, int fd, const char *restrict path, int flags, mode_t mode);
int posix_spawn_file_actions_addchdir_np(posix_spawn_file_actions_t *restrict fa, const char *restrict path);
int posix_spawnattr_init(posix_spawnattr_t *a);
int posix_spawnattr_destroy(posix_spawnattr_t *a);
int posix_spawnattr_setflags(posix_spawnattr_t *a, short flags);
int posix_spawnattr_getflags(const posix_spawnattr_t *restrict a, short *restrict flags);
int posix_spawnattr_setsigmask(posix_spawnattr_t *restrict a, const sigset_t *restrict mask);
int posix_spawnattr_setsigdefault(posix_spawnattr_t *restrict a, const sigset_t *restrict def);
int posix_spawnattr_setpgroup(posix_spawnattr_t *a, pid_t pgrp);

#endif
