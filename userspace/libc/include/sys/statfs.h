// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// File system statistics (musl's layout)

#ifndef _SYS_STATFS_H
#define _SYS_STATFS_H

#include <sys/types.h>

typedef struct { int __val[2]; } fsid_t;
struct statfs {
    unsigned long f_type, f_bsize;
    unsigned long f_blocks, f_bfree, f_bavail, f_files, f_ffree;
    fsid_t f_fsid;
    unsigned long f_namelen, f_frsize, f_flags, f_spare[4];
};

#define TMPFS_MAGIC 0x01021994
#define RAMFS_MAGIC 0x858458f6

int statfs(const char *path, struct statfs *s);
int fstatfs(int fd, struct statfs *s);

#endif
