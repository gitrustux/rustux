// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <sys/types.h>

#define S_IFMT   0170000
#define S_IFDIR  0040000
#define S_IFREG  0100000
#define S_IFIFO  0010000
#define S_IFCHR  0020000
#define S_ISDIR(m)  (((m) & S_IFMT) == S_IFDIR)
#define S_ISREG(m)  (((m) & S_IFMT) == S_IFREG)
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISCHR(m)  (((m) & S_IFMT) == S_IFCHR)

#ifndef _STRUCT_TIMESPEC
#define _STRUCT_TIMESPEC
struct timespec { time_t tv_sec; long tv_nsec; };
#endif

struct stat {
    ino_t st_ino;
    mode_t st_mode;     // type and permission bits
    nlink_t st_nlink;
    uid_t st_uid;
    gid_t st_gid;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    struct timespec st_mtim;
};
#define st_mtime st_mtim.tv_sec

int stat(const char *restrict path, struct stat *restrict st);
int lstat(const char *restrict path, struct stat *restrict st);
int fstat(int fd, struct stat *st);
int mkdir(const char *path, mode_t mode);
mode_t umask(mode_t mask);

#endif
