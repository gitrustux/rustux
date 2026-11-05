// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _FCNTL_H
#define _FCNTL_H

#include <sys/types.h>

#define O_RDONLY    0x0
#define O_WRONLY    0x1
#define O_RDWR      0x2
#define O_ACCMODE   0x3
#define O_CREAT     0x40
#define O_EXCL      0x80
#define O_NOCTTY    0x100
#define O_TRUNC     0x200
#define O_APPEND    0x400
#define O_NONBLOCK  0x800
#define O_DIRECTORY 0x10000
#define O_CLOEXEC   0x80000

#define O_NOFOLLOW  0x20000
#define O_PATH      0x200000
#define O_LARGEFILE 0
#define FD_CLOEXEC  1
#define F_DUPFD 0
#define F_GETFD 1
#define F_SETFD 2
#define F_GETFL 3
#define F_SETFL 4
#define F_DUPFD_CLOEXEC 1030
#define AT_FDCWD (-100)
#define AT_SYMLINK_NOFOLLOW 0x100
#define AT_REMOVEDIR 0x200
#define AT_EMPTY_PATH 0x1000

int open(const char *path, int flags, ...);
int openat(int dirfd, const char *path, int flags, ...);
int fcntl(int fd, int cmd, ...);
int unlinkat(int dirfd, const char *path, int flags);
int renameat(int olddirfd, const char *old, int newdirfd, const char *new);
int creat(const char *path, mode_t mode);

#endif
