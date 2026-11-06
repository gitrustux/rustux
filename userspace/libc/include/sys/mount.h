// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Mounting filesystems (step 14). A filesystem is mounted by starting its
// server (fatfs) with a channel and handing fsd the other end with
// rx_mount; the mount tool does both. docs/FS.md "Mounts".

#ifndef _SYS_MOUNT_H
#define _SYS_MOUNT_H

#include <rustux/fs.h>
#include <rustux/syscalls.h>

#define MS_RDONLY 1
#define MNT_FORCE 1

// Attach the filesystem whose root directory connection is `root` (consumed)
// at directory `target`; source and type are what /etc/mtab shows.
// flags: RX_FS_MOUNT_RDONLY. 0 or a negative errno.
int rx_mount(const char *source, const char *target, const char *type, unsigned flags, rx_handle_t root);
// Every mounted filesystem written out and marked clean, the root last
// (poweroff, reboot)
int rx_fs_shutdown_all(void);
int umount(const char *target);
int umount2(const char *target, int flags);
// The statfs data of an open file's filesystem (0 or a negative errno)
int rx_statfs_fd(int fd, rx_fs_statfs_t *out);

#endif
