// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Files and directories through the filesystem service (protocol in
// <rustux/fs.h>). Every path is made absolute against the current
// directory and resolved lexically ("." and ".." are removed here), then
// sent to the namespace root. The current directory is a path, kept in
// $PWD so programs started from here begin in the same place.

#include "libc.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <rustux/fs.h>
#include <rustux/service.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

static rx_handle_t ns_root;
static char cwd[PATH_MAX] = "/";
static uint32_t next_txid = 1;

rx_handle_t rx_ns_root(void) { return ns_root; }

void rx_ns_set_root(rx_handle_t root) {
    if (ns_root) rx_handle_close(ns_root);
    ns_root = root;
}

void __fs_init(void) {
    const char *pwd = getenv("PWD");
    if (pwd && pwd[0] == '/' && strlen(pwd) < sizeof cwd) strcpy(cwd, pwd);
}

// One request at a time per process: the message buffer is shared, and a
// reply must reach the thread that sent the request
static volatile int fs_lock;

// Applied to the modes of new files and directories (as the kernel does
// on Unix); permissions are recorded, not enforced, until step 17
static mode_t cur_umask = 022;

static int fs_call(rx_handle_t ch, uint32_t op, const void *a, size_t alen, const void *b, size_t blen,
                   rx_handle_t h, void *out, size_t cap, size_t *len, rx_handle_t *hout);

int __fs_call(rx_handle_t ch, uint32_t op, const void *a, size_t alen, const void *b, size_t blen,
              rx_handle_t h, void *out, size_t cap, size_t *len) {
    __lock(&fs_lock);
    int r = fs_call(ch, op, a, alen, b, blen, h, out, cap, len, 0);
    __unlock(&fs_lock);
    return r;
}

int __fs_get_vmo(int fd, rx_handle_t *vmo, uint64_t *offset, uint64_t *size) {
    struct fd *f = __fd_get(fd);
    if (!f || f->kind != RX_FD_FILE) return -EBADF;
    uint64_t v[2];
    size_t n = 0;
    *vmo = 0;
    __lock(&fs_lock);
    int r = fs_call(f->handle, RX_FS_OP_GET_VMO, 0, 0, 0, 0, 0, v, sizeof v, &n, vmo);
    __unlock(&fs_lock);
    if (r == 0 && (n < sizeof v || !*vmo)) r = -EIO;
    if (r < 0) {
        if (*vmo) rx_handle_close(*vmo);
        *vmo = 0;
        return r;
    }
    *offset = v[0];
    *size = v[1];
    return 0;
}

static int fs_call(rx_handle_t ch, uint32_t op, const void *a, size_t alen, const void *b, size_t blen,
                   rx_handle_t h, void *out, size_t cap, size_t *len, rx_handle_t *hout) {
    static char msg[sizeof(rx_fs_hdr_t) + RX_FS_MAX_IO + PATH_MAX + 64];
    if (len) *len = 0;
    if (!ch) {
        if (h) rx_handle_close(h);
        return -ENOENT; // no filesystem (yet)
    }
    if (sizeof(rx_fs_hdr_t) + alen + blen > sizeof msg) {
        if (h) rx_handle_close(h);
        return -E2BIG;
    }
    rx_fs_hdr_t hdr = { next_txid++, op };
    memcpy(msg, &hdr, sizeof hdr);
    if (alen) memcpy(msg + sizeof hdr, a, alen);
    if (blen) memcpy(msg + sizeof hdr + alen, b, blen);
    rx_status_t r = rx_channel_write(ch, 0, msg, (uint32_t)(sizeof hdr + alen + blen), h ? &h : 0, h ? 1 : 0);
    if (r != RX_OK) return r == RX_ERR_PEER_CLOSED ? -EIO : -rx_status_to_errno(r);
    uint32_t nb = 0;
    for (;;) {
        uint32_t observed = 0;
        rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &observed);
        rx_handle_t got = 0;
        uint32_t nh = 0;
        r = rx_channel_read(ch, 0, msg, &got, sizeof msg, 1, &nb, &nh);
        if (r == RX_ERR_SHOULD_WAIT) continue;
        if (r != RX_OK) return -EIO;
        if (nb >= sizeof(rx_fs_reply_t) && ((rx_fs_reply_t *)msg)->txid == hdr.txid) {
            if (nh && hout) *hout = got;
            else if (nh) rx_handle_close(got);
            break;
        }
        if (nh) rx_handle_close(got);
        // A reply to someone else's request (a connection shared with
        // another process): dropped
    }
    rx_fs_reply_t rep;
    memcpy(&rep, msg, sizeof rep);
    size_t n = nb - sizeof rep;
    if (n > cap) n = cap;
    if (out && n) memcpy(out, msg + sizeof rep, n);
    if (len) *len = n;
    return rep.status;
}

// `path` made absolute and lexically resolved, without the leading '/'
// ("" is the root). Returns 0 or a negative errno.
static int resolve(const char *path, char *out) {
    char tmp[PATH_MAX * 2];
    if (!path || !*path) return -ENOENT;
    if (path[0] == '/') {
        if (strlen(path) >= PATH_MAX) return -ENAMETOOLONG;
        strcpy(tmp, path);
    } else {
        if (strlen(cwd) + 1 + strlen(path) >= sizeof tmp) return -ENAMETOOLONG;
        strcpy(tmp, cwd);
        strcat(tmp, "/");
        strcat(tmp, path);
    }
    size_t len = 0;
    char *save, *part = strtok_r(tmp, "/", &save);
    out[0] = 0;
    for (; part; part = strtok_r(0, "/", &save)) {
        if (!strcmp(part, ".")) continue;
        if (!strcmp(part, "..")) {
            while (len > 0 && out[len - 1] != '/') len--;
            if (len > 0) len--; // the '/' before it
            out[len] = 0;
            continue;
        }
        size_t k = strlen(part);
        if (k > NAME_MAX) return -ENAMETOOLONG;
        if (len + (len ? 1 : 0) + k >= PATH_MAX) return -ENAMETOOLONG;
        if (len) out[len++] = '/';
        memcpy(out + len, part, k);
        len += k;
        out[len] = 0;
    }
    return 0;
}

static int fail(int neg_errno) {
    errno = -neg_errno;
    return -1;
}

int open(const char *path, int flags, ...) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return fail(r);
    uint32_t mode = 0644;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, unsigned int) & ~cur_umask;
        va_end(ap);
    }
    int fd = __fd_alloc(0);
    if (fd < 0) return -1;
    rx_handle_t c, s;
    if (rx_channel_create(0, &c, &s) != RX_OK) return fail(-ENOMEM);
    // Only these flags mean something to the filesystem service
    uint32_t req[2] = { (uint32_t)flags & (O_ACCMODE | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_DIRECTORY), mode };
    uint32_t type = 0;
    r = __fs_call(ns_root, RX_FS_OP_OPEN, req, sizeof req, rel, strlen(rel), s, &type, sizeof type, 0);
    if (r < 0) {
        rx_handle_close(c);
        return fail(r);
    }
    if (type == RX_FS_TYPE_SERVICE) {
        // A service speaks its own protocol, not files: rx_service_connect
        rx_handle_close(c);
        return fail(-ENXIO);
    }
    __fds[fd].kind = RX_FD_FILE;
    __fds[fd].handle = c;
    __fds[fd].flags = flags & (O_ACCMODE | O_APPEND | O_NONBLOCK);
    __fds[fd].cloexec = (flags & O_CLOEXEC) != 0;
    __fds[fd].path = malloc(strlen(rel) + 2);
    if (__fds[fd].path) {
        __fds[fd].path[0] = '/';
        strcpy(__fds[fd].path + 1, rel);
    }
    return fd;
}

int creat(const char *path, mode_t mode) {
    return open(path, O_WRONLY | O_CREAT | O_TRUNC, mode);
}

static void fill_stat(const rx_fs_attr_t *a, struct stat *st) {
    memset(st, 0, sizeof *st);
    st->st_ino = a->ino;
    st->st_dev = a->fsid;
    st->st_mode = (a->type == RX_FS_TYPE_DIR ? S_IFDIR : a->type == RX_FS_TYPE_CHR ? S_IFCHR
                   : a->type == RX_FS_TYPE_SERVICE ? S_IFSOCK : S_IFREG) | (a->mode & 07777);
    st->st_nlink = a->nlink;
    st->st_size = (off_t)a->size;
    st->st_blksize = 4096;
    st->st_blocks = (blkcnt_t)((a->size + 511) / 512);
    st->st_mtim.tv_sec = a->mtime_ns / 1000000000;
    st->st_mtim.tv_nsec = a->mtime_ns % 1000000000;
    st->st_atim = st->st_ctim = st->st_mtim;
}

static int stat_at(const char *path, rx_fs_attr_t *a) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return r;
    return __fs_call(ns_root, RX_FS_OP_STAT_AT, rel, strlen(rel), 0, 0, 0, a, sizeof *a, 0);
}

int stat(const char *restrict path, struct stat *restrict st) {
    rx_fs_attr_t a;
    int r = stat_at(path, &a);
    if (r < 0) return fail(r);
    fill_stat(&a, st);
    return 0;
}

int lstat(const char *restrict path, struct stat *restrict st) {
    return stat(path, st); // no symbolic links yet
}

int fstat(int fd, struct stat *st) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind == RX_FD_FILE) {
        rx_fs_attr_t a;
        int r = __fs_call(f->handle, RX_FS_OP_STAT, 0, 0, 0, 0, 0, &a, sizeof a, 0);
        if (r < 0) return fail(r);
        fill_stat(&a, st);
        return 0;
    }
    memset(st, 0, sizeof *st);
    st->st_mode = (f->kind == RX_FD_PIPE ? S_IFIFO : S_IFCHR) | 0620;
    st->st_nlink = 1;
    return 0;
}

int access(const char *path, int mode) {
    rx_fs_attr_t a;
    int r = stat_at(path, &a);
    if (r < 0) return fail(r);
    if ((mode & W_OK) && a.read_only) return fail(-EROFS);
    if ((mode & X_OK) && !(a.mode & 0111)) return fail(-EACCES);
    return 0;
}

int mkdir(const char *path, mode_t mode) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return fail(r);
    uint32_t m = mode & ~cur_umask;
    r = __fs_call(ns_root, RX_FS_OP_MKDIR, &m, sizeof m, rel, strlen(rel), 0, 0, 0, 0);
    return r < 0 ? fail(r) : 0;
}

static int remove_path(const char *path, uint32_t flags) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return fail(r);
    r = __fs_call(ns_root, RX_FS_OP_UNLINK, &flags, sizeof flags, rel, strlen(rel), 0, 0, 0, 0);
    return r < 0 ? fail(r) : 0;
}

int unlink(const char *path) { return remove_path(path, 0); }
int rmdir(const char *path) { return remove_path(path, RX_FS_UNLINK_DIR); }

int remove(const char *path) {
    struct stat st;
    if (stat(path, &st) < 0) return -1;
    return S_ISDIR(st.st_mode) ? rmdir(path) : unlink(path);
}

int rename(const char *old, const char *new) {
    char a[PATH_MAX], b[PATH_MAX];
    int r = resolve(old, a);
    if (r == 0) r = resolve(new, b);
    if (r < 0) return fail(r);
    // old_len, old path, new path
    char req[4 + PATH_MAX];
    uint32_t n = (uint32_t)strlen(a);
    memcpy(req, &n, 4);
    memcpy(req + 4, a, n);
    r = __fs_call(ns_root, RX_FS_OP_RENAME, req, 4 + n, b, strlen(b), 0, 0, 0, 0);
    return r < 0 ? fail(r) : 0;
}

int chdir(const char *path) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return fail(r);
    rx_fs_attr_t a;
    r = __fs_call(ns_root, RX_FS_OP_STAT_AT, rel, strlen(rel), 0, 0, 0, &a, sizeof a, 0);
    if (r < 0) return fail(r);
    if (a.type != RX_FS_TYPE_DIR) return fail(-ENOTDIR);
    cwd[0] = '/';
    strcpy(cwd + 1, rel);
    setenv("PWD", cwd, 1);
    return 0;
}

char *getcwd(char *buf, size_t size) {
    size_t n = strlen(cwd) + 1;
    if (!buf) {
        buf = malloc(size > n ? size : n);
        if (!buf) { errno = ENOMEM; return 0; }
        size = size > n ? size : n;
    }
    if (n > size) {
        errno = ERANGE;
        return 0;
    }
    memcpy(buf, cwd, n);
    return buf;
}

int ftruncate(int fd, off_t length) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE || length < 0) return fail(-EINVAL);
    uint64_t len = (uint64_t)length;
    int r = __fs_call(f->handle, RX_FS_OP_TRUNCATE, &len, sizeof len, 0, 0, 0, 0, 0, 0);
    return r < 0 ? fail(r) : 0;
}

int truncate(const char *path, off_t length) {
    int fd = open(path, O_WRONLY);
    if (fd < 0) return -1;
    int r = ftruncate(fd, length);
    int e = errno;
    close(fd);
    errno = e;
    return r;
}

int fsync(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE) return 0;
    int r = __fs_call(f->handle, RX_FS_OP_SYNC, 0, 0, 0, 0, 0, 0, 0, 0);
    return r < 0 ? fail(r) : 0;
}

mode_t umask(mode_t mask) {
    mode_t old = cur_umask;
    cur_umask = mask & 0777;
    return old;
}

// ---- directories ----

struct _DIR {
    int fd;
    uint32_t len, pos;
    struct dirent ent;
    char buf[4096];
};

DIR *opendir(const char *path) {
    int fd = open(path, O_RDONLY | O_DIRECTORY);
    if (fd < 0) return 0;
    DIR *d = calloc(1, sizeof *d);
    if (!d) {
        close(fd);
        errno = ENOMEM;
        return 0;
    }
    d->fd = fd;
    return d;
}

struct dirent *readdir(DIR *d) {
    if (d->pos >= d->len) {
        struct fd *f = __fd_get(d->fd);
        if (!f) return 0;
        uint32_t max = sizeof d->buf;
        size_t n = 0;
        int r = __fs_call(f->handle, RX_FS_OP_READDIR, &max, sizeof max, 0, 0, 0, d->buf, sizeof d->buf, &n);
        if (r < 0) { errno = -r; return 0; }
        if (n == 0) return 0;
        d->len = (uint32_t)n;
        d->pos = 0;
    }
    char *p = d->buf + d->pos;
    uint64_t ino;
    memcpy(&ino, p, 8);
    uint8_t type = (uint8_t)p[8], nlen = (uint8_t)p[9];
    d->ent.d_ino = ino;
    d->ent.d_off = 0;
    d->ent.d_reclen = sizeof d->ent;
    d->ent.d_type = type == RX_FS_TYPE_DIR ? DT_DIR : type == RX_FS_TYPE_CHR ? DT_CHR
                  : type == RX_FS_TYPE_SERVICE ? DT_SOCK : DT_REG;
    memcpy(d->ent.d_name, p + 10, nlen);
    d->ent.d_name[nlen] = 0;
    d->pos += 10u + nlen;
    return &d->ent;
}

void rewinddir(DIR *d) {
    struct fd *f = __fd_get(d->fd);
    if (f) __fs_call(f->handle, RX_FS_OP_REWIND, 0, 0, 0, 0, 0, 0, 0, 0);
    d->len = d->pos = 0;
}

int closedir(DIR *d) {
    int r = close(d->fd);
    free(d);
    return r;
}

int dirfd(DIR *d) { return d->fd; }

DIR *fdopendir(int fd) {
    struct stat st;
    if (fstat(fd, &st) < 0) return 0;
    if (!S_ISDIR(st.st_mode)) {
        errno = ENOTDIR;
        return 0;
    }
    DIR *d = calloc(1, sizeof *d);
    if (!d) {
        errno = ENOMEM;
        return 0;
    }
    d->fd = fd;
    return d;
}

// ---- *at() calls: a relative path is taken from the directory's path ----

// `path` relative to `dirfd` as a path the calls above take; 0 or -1
static int at_path(int dirfd, const char *path, char *out) {
    if (!path) return fail(-EFAULT);
    if (path[0] == '/' || dirfd == AT_FDCWD) {
        if (strlen(path) >= PATH_MAX) return fail(-ENAMETOOLONG);
        strcpy(out, path);
        return 0;
    }
    struct fd *f = __fd_get(dirfd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE || !f->path) return fail(-ENOTDIR);
    if (strlen(f->path) + 1 + strlen(path) >= PATH_MAX) return fail(-ENAMETOOLONG);
    strcpy(out, f->path);
    strcat(out, "/");
    strcat(out, path);
    return 0;
}

int openat(int dirfd, const char *path, int flags, ...) {
    char p[PATH_MAX];
    unsigned mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = va_arg(ap, unsigned int);
        va_end(ap);
    }
    if (at_path(dirfd, path, p) < 0) return -1;
    return open(p, flags, mode);
}

int fstatat(int dirfd, const char *restrict path, struct stat *restrict st, int flags) {
    if ((flags & AT_EMPTY_PATH) && (!path || !*path)) return fstat(dirfd, st);
    char p[PATH_MAX];
    if (at_path(dirfd, path, p) < 0) return -1;
    return stat(p, st);
}

int unlinkat(int dirfd, const char *path, int flags) {
    char p[PATH_MAX];
    if (at_path(dirfd, path, p) < 0) return -1;
    return (flags & AT_REMOVEDIR) ? rmdir(p) : unlink(p);
}

int mkdirat(int dirfd, const char *path, mode_t mode) {
    char p[PATH_MAX];
    if (at_path(dirfd, path, p) < 0) return -1;
    return mkdir(p, mode);
}

int renameat(int olddirfd, const char *old, int newdirfd, const char *new) {
    char a[PATH_MAX], b[PATH_MAX];
    if (at_path(olddirfd, old, a) < 0 || at_path(newdirfd, new, b) < 0) return -1;
    return rename(a, b);
}

int faccessat(int dirfd, const char *path, int mode, int flags) {
    (void)flags;
    char p[PATH_MAX];
    if (at_path(dirfd, path, p) < 0) return -1;
    return access(p, mode);
}

int fchdir(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE || !f->path) return fail(-ENOTDIR);
    return chdir(f->path);
}

char *realpath(const char *restrict path, char *restrict resolved) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) { fail(r); return 0; }
    struct stat st;
    if (stat(path, &st) < 0) return 0;
    if (strlen(rel) + 2 > PATH_MAX) { fail(-ENAMETOOLONG); return 0; }
    if (!resolved && !(resolved = malloc(PATH_MAX))) { fail(-ENOMEM); return 0; }
    resolved[0] = '/';
    strcpy(resolved + 1, rel);
    return resolved;
}

// No symbolic links yet
ssize_t readlink(const char *restrict path, char *restrict buf, size_t size) {
    (void)buf; (void)size;
    struct stat st;
    if (stat(path, &st) < 0) return -1;
    return fail(-EINVAL);
}
int symlink(const char *target, const char *link) { (void)target; (void)link; return fail(-EPERM); }
int link(const char *old, const char *new) { (void)old; (void)new; return fail(-EPERM); }

// chmod and file times: users.c

// ---- services (/svc, step 12) ----

int rx_service_connect(const char *path, rx_handle_t *out) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) return r;
    rx_handle_t c, s;
    if (rx_channel_create(0, &c, &s) != RX_OK) return -ENOMEM;
    uint32_t req[2] = { O_RDWR, 0 };
    uint32_t type = 0;
    r = __fs_call(ns_root, RX_FS_OP_OPEN, req, sizeof req, rel, strlen(rel), s, &type, sizeof type, 0);
    if (r < 0) {
        rx_handle_close(c);
        return r;
    }
    if (type != RX_FS_TYPE_SERVICE) {
        rx_handle_close(c);
        return -ENOTSOCK;
    }
    *out = c;
    return 0;
}

int rx_service_publish(const char *path, rx_handle_t server) {
    char rel[PATH_MAX];
    int r = resolve(path, rel);
    if (r < 0) {
        rx_handle_close(server);
        return r;
    }
    return __fs_call(ns_root, RX_FS_OP_SERVE, 0, 0, rel, strlen(rel), server, 0, 0, 0);
}

// ---- mounts (step 14, <sys/mount.h>) ----

int rx_mount(const char *source, const char *target, const char *type, unsigned flags, rx_handle_t root) {
    char rel[PATH_MAX], req[sizeof(rx_fs_mount_t) + 256 + 32];
    int r = resolve(target, rel);
    size_t sl = strlen(source), tl = strlen(type);
    if (r == 0 && (sl > 255 || tl > 31)) r = -EINVAL;
    if (r < 0) {
        rx_handle_close(root);
        return r;
    }
    rx_fs_mount_t m = { flags, (uint16_t)sl, (uint16_t)tl };
    memcpy(req, &m, sizeof m);
    memcpy(req + sizeof m, source, sl);
    memcpy(req + sizeof m + sl, type, tl);
    return __fs_call(ns_root, RX_FS_OP_MOUNT, req, sizeof m + sl + tl, rel, strlen(rel), root, 0, 0, 0);
}

int umount2(const char *target, int flags) {
    char rel[PATH_MAX];
    int r = resolve(target, rel);
    uint32_t f = (flags & MNT_FORCE) ? RX_FS_UNMOUNT_FORCE : 0;
    if (r == 0) r = __fs_call(ns_root, RX_FS_OP_UNMOUNT, &f, 4, rel, strlen(rel), 0, 0, 0, 0);
    if (r < 0) {
        errno = -r;
        return -1;
    }
    return 0;
}

int umount(const char *target) { return umount2(target, 0); }

// Every mounted filesystem writes out what it has (fsd forwards SYNC)
void sync(void) {
    if (ns_root) __fs_call(ns_root, RX_FS_OP_SYNC, 0, 0, 0, 0, 0, 0, 0, 0);
}

int rx_statfs_fd(int fd, rx_fs_statfs_t *out) {
    struct fd *f = __fd_get(fd);
    if (!f || f->kind != RX_FD_FILE) return -EBADF;
    return __fs_call(f->handle, RX_FS_OP_STATFS, 0, 0, 0, 0, 0, out, sizeof *out, 0);
}
