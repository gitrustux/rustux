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
#define S_IFLNK  0120000
#define S_IFBLK  0060000
#define S_IFSOCK 0140000
#define S_ISLNK(m)  (((m) & S_IFMT) == S_IFLNK)
#define S_ISDIR(m)  (((m) & S_IFMT) == S_IFDIR)
#define S_ISREG(m)  (((m) & S_IFMT) == S_IFREG)
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISCHR(m)  (((m) & S_IFMT) == S_IFCHR)

#ifndef _STRUCT_TIMESPEC
#define _STRUCT_TIMESPEC
struct timespec { time_t tv_sec; long tv_nsec; };
#endif

// The musl x86_64 layout (Rust's std reads it through the libc crate)
struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    unsigned int __pad0;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    struct timespec st_atim;
    struct timespec st_mtim;
    struct timespec st_ctim;
    long __unused[3];
};
#define st_atime st_atim.tv_sec
#define st_ctime st_ctim.tv_sec
#define st_mtime st_mtim.tv_sec

int stat(const char *restrict path, struct stat *restrict st);
int lstat(const char *restrict path, struct stat *restrict st);
int fstat(int fd, struct stat *st);
int fstatat(int dirfd, const char *restrict path, struct stat *restrict st, int flags);
int mkdirat(int dirfd, const char *path, mode_t mode);
int chmod(const char *path, mode_t mode);
int fchmod(int fd, mode_t mode);
int fchmodat(int dirfd, const char *path, mode_t mode, int flags);
int utimensat(int dirfd, const char *path, const struct timespec t[2], int flags);
int futimens(int fd, const struct timespec t[2]);
int mkfifo(const char *path, mode_t mode);
int mknod(const char *path, mode_t mode, dev_t dev);
#define UTIME_NOW  0x3fffffff
#define UTIME_OMIT 0x3ffffffe
int mkdir(const char *path, mode_t mode);
mode_t umask(mode_t mask);

#endif
