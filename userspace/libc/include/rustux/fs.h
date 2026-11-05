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

#define RX_FS_UNLINK_DIR 1u

#define RX_FS_TYPE_FILE 1u
#define RX_FS_TYPE_DIR  2u

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
    uint32_t pad;
} rx_fs_attr_t;

#endif
