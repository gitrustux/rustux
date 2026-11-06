// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// File protocol between programs and the filesystem service (fsd), over
// channels. Each open file or directory is its own channel ("connection").
// A request is one message: rx_fs_hdr_t then the op's fields; the reply
// is one message on the same channel: rx_fs_reply_t then the op's fields.
// status is 0 or a negative errno value. Paths are relative to the
// directory the connection is open on, "/"-separated, without "." or ".."
// (the C library resolves those). See docs/FS.md.

#ifndef RUSTUX_FS_H
#define RUSTUX_FS_H

#include <stdint.h>

typedef struct { uint32_t txid; uint32_t op; } rx_fs_hdr_t;
typedef struct { uint32_t txid; int32_t status; } rx_fs_reply_t;

// On a directory. Request: u32 flags (O_*), u32 mode, path; one handle:
// the server end of the new connection. Reply: u32 type (RX_FS_TYPE_*).
#define RX_FS_OP_OPEN      1
// Request: one handle, the server end of a new connection to the same open
// file (sharing its offset, like dup() across processes). Reply: nothing.
#define RX_FS_OP_CLONE     2
// Reply: rx_fs_attr_t
#define RX_FS_OP_STAT      3
// Request: u32 count (at most RX_FS_MAX_IO). Reply: the bytes (fewer at
// end of file, none past it). Advances the offset.
#define RX_FS_OP_READ      4
// Request: the bytes (at most RX_FS_MAX_IO). Reply: u64 written.
#define RX_FS_OP_WRITE     5
// Request: i64 offset, u32 whence (SEEK_*). Reply: u64 new offset.
#define RX_FS_OP_SEEK      6
// Request: u64 length.
#define RX_FS_OP_TRUNCATE  7
// On a directory. Request: u32 max bytes. Reply: entries
// { u64 ino; u8 type; u8 name_len; name } back to back; none at the end.
#define RX_FS_OP_READDIR   8
// On a directory: start READDIR from the first entry again
#define RX_FS_OP_REWIND    9
// On a directory. Request: u32 mode, path.
#define RX_FS_OP_MKDIR    10
// On a directory. Request: u32 flags (RX_FS_UNLINK_DIR for rmdir), path.
#define RX_FS_OP_UNLINK   11
// On a directory. Request: u32 old_len, old path, new path (both relative
// to this directory).
#define RX_FS_OP_RENAME   12
#define RX_FS_OP_SYNC     13
// On a directory. Request: path. Reply: rx_fs_attr_t of that path.
#define RX_FS_OP_STAT_AT  14
// On any. Request: rx_fs_setattr_t. Reply: nothing. Changes the
// permission bits and/or the modification time (step 11).
#define RX_FS_OP_SETATTR  15

// On a file. Request: nothing. Reply: u64 offset, u64 size, and one
// handle: a read-only VMO holding the file's bytes at that offset. Only
// files of the boot image have one (others: ENOTSUP). The program loader
// maps programs from it instead of reading them (step 11).
#define RX_FS_OP_GET_VMO  16

// On a directory (step 12). Request: path; one handle: a channel to a
// server. Makes a service node there (replacing an older service of that
// name). An OPEN of a service sends the opener's connection to the server
// as an rx_svc_connect_t message carrying the handle, and replies
// RX_FS_TYPE_SERVICE; from then on the opener talks to the server's own
// protocol. ECONNREFUSED if the server has gone. Services live in /svc.
#define RX_FS_OP_SERVE    17

// Mounts (step 14). A filesystem other than fsd's own (FAT on a disk,
// served by fatfs) is attached at a directory of fsd's tree; fsd then
// forwards the path requests that go below it (OPEN, STAT_AT, MKDIR,
// UNLINK, RENAME) to that filesystem's root connection, and the opened
// files talk to its server directly.
//
// On a directory (fsd). Request: rx_fs_mount_t, the source (device name),
// the type ("vfat"), the path; one handle: a connection to the root
// directory of the filesystem. Reply: nothing. The path must be an
// existing directory; EBUSY if something is mounted there.
#define RX_FS_OP_MOUNT    18
typedef struct {
    uint32_t flags;         // RX_FS_MOUNT_*
    uint16_t source_len, type_len;
} rx_fs_mount_t;
#define RX_FS_MOUNT_RDONLY 1u
// On a directory (fsd). Request: u32 flags (RX_FS_UNMOUNT_FORCE), path.
// fsd asks the filesystem to stop (SHUTDOWN) and detaches it. EINVAL: not
// a mount point; EBUSY: files are still open on it (without FORCE).
#define RX_FS_OP_UNMOUNT  19
#define RX_FS_UNMOUNT_FORCE 1u
// Sent by fsd to a mounted filesystem's root connection: write everything
// out and end once its last connection closes. Request: u32 flags
// (RX_FS_UNMOUNT_FORCE: even with files open). Reply: nothing, or EBUSY.
#define RX_FS_OP_SHUTDOWN 20
// Any connection. Reply: rx_fs_statfs_t (statfs, df)
#define RX_FS_OP_STATFS   21

// Symbolic and hard links (step 15). On a directory.
// SYMLINK: u32 target_len, target, path: makes a symbolic link at path.
// READLINK: path; reply: the target (EINVAL if path is not a link).
// LINK: u32 old_len, old path, new path: a second name for a file (same
//   filesystem, else EXDEV; EPERM for directories and where links do not
//   exist, as on FAT).
// LSTAT_AT: as STAT_AT, but a link at the end of the path is not followed.
#define RX_FS_OP_SYMLINK  22
#define RX_FS_OP_READLINK 23
#define RX_FS_OP_LINK     24
#define RX_FS_OP_LSTAT_AT 25
// The final step of a shutdown, on fsd's root (poweroff, reboot): every
// mounted filesystem writes everything out and is marked clean, the root
// filesystem last; from then on writes to them fail (EROFS). Request:
// nothing. Reply: nothing.
#define RX_FS_OP_SHUTDOWN_ALL 26

// Symbolic links are followed by the one that serves the path. A server
// that meets a link it must follow (anywhere but at the end of the path,
// and at the end for OPEN without O_NOFOLLOW and STAT_AT) replies with
// this positive status and: u8 which (0: the request's path, 1: the second
// path of RENAME or LINK), then the path to use instead. If that path
// starts with '/', it is from the namespace root; otherwise it is from the
// server's own root (fsd puts the mount point in front). fsd normalises
// it (".", "..") and starts again, at most 40 times (then ELOOP). An OPEN
// that is redirected sends the connection's server end back with the
// reply.
#define RX_FS_REDIRECT 1
#define RX_FS_MAX_LINKS 40
typedef struct {
    uint64_t block_size;
    uint64_t blocks, blocks_free;
    uint64_t files, files_free;  // 0 if the filesystem has no such limit
    uint32_t name_max;
    uint32_t flags;              // RX_FS_MOUNT_RDONLY
    char type[16];               // "tmpfs", "bootfs", "vfat", ...
} rx_fs_statfs_t;

typedef struct {
    uint32_t op;        // RX_SVC_CONNECT
    uint32_t flags;     // the opener's O_* flags
} rx_svc_connect_t;
#define RX_SVC_CONNECT 1u

#define RX_FS_SETATTR_MODE  1u
#define RX_FS_SETATTR_MTIME 2u
#define RX_FS_SETATTR_UID   4u   // step 15
#define RX_FS_SETATTR_GID   8u
#define RX_FS_SETATTR_ATIME 16u
typedef struct {
    uint32_t mask;      // RX_FS_SETATTR_*
    uint32_t mode;      // permission bits (07777)
    int64_t mtime_ns;   // nanoseconds since 1970 (UTC)
    uint32_t uid, gid;  // owner and group
    int64_t atime_ns;   // last access
} rx_fs_setattr_t;

#define RX_FS_UNLINK_DIR 1u

#define RX_FS_TYPE_FILE 1u
#define RX_FS_TYPE_DIR  2u
#define RX_FS_TYPE_CHR  3u   // a device (/dev/null, /dev/zero)
#define RX_FS_TYPE_SERVICE 4u // a service (RX_FS_OP_SERVE): S_IFSOCK
#define RX_FS_TYPE_SYMLINK 5u // a symbolic link (step 15): S_IFLNK

// Most data in one READ or WRITE
#define RX_FS_MAX_IO 32768u

typedef struct {
    uint32_t type;      // RX_FS_TYPE_*
    uint32_t mode;      // permission bits (0755, 0644, ...)
    uint64_t size;
    uint64_t ino;
    uint64_t nlink;
    int64_t mtime_ns;   // monotonic clock until step 9 brings real time
    uint32_t read_only; // on a read-only filesystem (bootfs)
    uint32_t fsid;      // which filesystem (1 tmpfs, 2 bootfs, 3 devices): st_dev
    // Step 15 (0 where a filesystem has none: then the C library uses
    // mtime for both times and the size for the blocks)
    uint32_t uid, gid;  // owner and group
    int64_t atime_ns;   // last access
    int64_t ctime_ns;   // last change of the inode
    uint64_t blocks;    // space used, in 512-byte units
} rx_fs_attr_t;

#endif
