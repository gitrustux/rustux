// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// perm.h: Unix permission rules, shared by the filesystem servers (fsd,
// ext2fs, fatfs) (step 17). `who` is a request's sender (or, forwarded by
// fsd, the one it forwards for).

#ifndef PERM_H
#define PERM_H

#include <rustux/fs.h>
#include <rustux/syscalls.h>
#include <errno.h>
#include <string.h>

#define MAY_READ  4
#define MAY_WRITE 2
#define MAY_EXEC  1   // search, for directories

// May `who` do `want` (MAY_* bits) to a file with this mode and owner?
// root may do anything, but run a file only if some x bit is set.
static inline int perm_may(const rx_sender_t *who, uint32_t mode, uint32_t uid, uint32_t gid, int is_dir, int want) {
    if (who->euid == 0) return !(want & MAY_EXEC) || is_dir || (mode & 0111);
    uint32_t bits = who->euid == uid ? mode >> 6 : rx_sender_in_group(who, gid) ? mode >> 3 : mode;
    return ((int)(bits & 7) & want) == want;
}

// Removing or renaming a name in a sticky directory (/tmp): only the
// file's owner, the directory's owner, or root
static inline int perm_sticky_ok(const rx_sender_t *who, uint32_t dir_mode, uint32_t dir_uid, uint32_t file_uid) {
    return !(dir_mode & 01000) || who->euid == 0 || who->euid == dir_uid || who->euid == file_uid;
}

// chmod: the owner or root. A non-root owner who is not in the file's
// group loses the setgid bit.
static inline int perm_chmod(const rx_sender_t *who, uint32_t uid, uint32_t gid, uint32_t *mode) {
    if (who->euid != 0 && who->euid != uid) return -EPERM;
    if (who->euid != 0 && !rx_sender_in_group(who, gid)) *mode &= ~02000u;
    return 0;
}

// chown: changing the owner is root's; the owner may change the group to
// one it is in
static inline int perm_chown(const rx_sender_t *who, uint32_t uid, uint32_t new_uid, uint32_t new_gid, int set_uid, int set_gid) {
    if (who->euid == 0) return 0;
    if (set_uid && new_uid != uid) return -EPERM;
    if (set_gid && (who->euid != uid || !rx_sender_in_group(who, new_gid))) return -EPERM;
    return 0;
}

// Setting times: the owner or root; anyone who may write, to "now"
static inline int perm_utimes(const rx_sender_t *who, uint32_t mode, uint32_t uid, uint32_t gid, int to_now) {
    if (who->euid == 0 || who->euid == uid) return 0;
    return to_now && perm_may(who, mode, uid, gid, 0, MAY_WRITE) ? 0 : -EPERM;
}

// What open flags need
static inline int perm_open_want(uint32_t flags) {
    int acc = (int)(flags & 3), want = 0;
    if (acc == 0 || acc == 2) want |= MAY_READ;   // O_RDONLY, O_RDWR
    if (acc == 1 || acc == 2) want |= MAY_WRITE;  // O_WRONLY, O_RDWR
    if (flags & 01000) want |= MAY_WRITE;         // O_TRUNC
    return want;
}

// A request read with rx_channel_read_creds: if fsd forwarded it (the op
// has RX_FS_OP_AS), take the original sender from after the header and
// remove it from the message. -EPERM if a non-root sender set the bit.
static inline int perm_unwrap(char *msg, uint32_t *nb, rx_sender_t *who) {
    rx_fs_hdr_t h;
    if (*nb < sizeof h) return 0;
    memcpy(&h, msg, sizeof h);
    if (!(h.op & RX_FS_OP_AS)) return 0;
    if (who->euid != 0 || *nb < sizeof h + sizeof *who) return -EPERM;
    memcpy(who, msg + sizeof h, sizeof *who);
    memmove(msg + sizeof h, msg + sizeof h + sizeof *who, *nb - sizeof h - sizeof *who);
    *nb -= (uint32_t)sizeof *who;
    h.op &= ~RX_FS_OP_AS;
    memcpy(msg, &h, sizeof h);
    return 0;
}

static const rx_sender_t PERM_ROOT = { 0, 0, 0, 0, 0, 0, 0, { 0 } };

#endif
