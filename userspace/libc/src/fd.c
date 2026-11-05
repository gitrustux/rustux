// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// File descriptors. Each one is a handle plus a kind that says how to
// talk to it: the console (a channel to the console service), the debug
// log, a file or directory (a connection to the filesystem service) or a
// pipe (a socket).

#include "libc.h"
#include <errno.h>
#include <rustux/console.h>
#include <rustux/fs.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

struct fd __fds[MAX_FDS];

struct fd *__fd_get(int fd) {
    if (fd < 0 || fd >= MAX_FDS || __fds[fd].kind == RX_FD_NONE) {
        errno = EBADF;
        return 0;
    }
    return &__fds[fd];
}

int __fd_alloc(int from) {
    for (int i = from < 0 ? 0 : from; i < MAX_FDS; i++) {
        if (__fds[i].kind == RX_FD_NONE) return i;
    }
    errno = EMFILE;
    return -1;
}

rx_handle_t __fd_export(int fd) {
    struct fd *f = __fd_get(fd);
    rx_handle_t h = 0;
    if (!f) return 0;
    if (f->kind == RX_FD_FILE) {
        // A new connection sharing the open file (and its offset)
        rx_handle_t s;
        if (rx_channel_create(0, &h, &s) != RX_OK) return 0;
        if (__fs_call(f->handle, RX_FS_OP_CLONE, 0, 0, 0, 0, s, 0, 0, 0) < 0) {
            rx_handle_close(h);
            return 0;
        }
        return h;
    }
    if (rx_handle_duplicate(f->handle, RX_RIGHT_SAME_RIGHTS, &h) != RX_OK) return 0;
    return h;
}

static void release(struct fd *f) {
    if (f->handle) rx_handle_close(f->handle);
    free(f->rbuf);
    free(f->path);
    memset(f, 0, sizeof *f);
}

int rx_fd_bind(int fd, int kind, rx_handle_t h) {
    if (fd < 0 || fd >= MAX_FDS || kind == RX_FD_NONE) {
        errno = EINVAL;
        return -1;
    }
    release(&__fds[fd]);
    __fds[fd].kind = kind;
    __fds[fd].handle = h;
    return 0;
}

rx_handle_t rx_fd_handle(int fd) {
    struct fd *f = __fd_get(fd);
    return f ? f->handle : 0;
}

static ssize_t console_write(rx_handle_t h, const char *p, size_t n) {
    char msg[4 + RX_CONSOLE_MAX_DATA];
    size_t done = 0;
    uint32_t op = RX_CONSOLE_OP_WRITE;
    memcpy(msg, &op, 4);
    while (done < n) {
        size_t k = n - done;
        if (k > RX_CONSOLE_MAX_DATA) k = RX_CONSOLE_MAX_DATA;
        memcpy(msg + 4, p + done, k);
        rx_status_t r;
        // The console's queue can fill up; wait for it to drain
        while ((r = rx_channel_write(h, 0, msg, (uint32_t)(4 + k), 0, 0)) == RX_ERR_SHOULD_WAIT) rx_yield();
        if (r != RX_OK) return done ? (ssize_t)done : __set_errno_status(r);
        done += k;
    }
    return (ssize_t)done;
}

static ssize_t console_read(struct fd *f, char *p, size_t n) {
    if (n == 0) return 0;
    // Raw mode with VMIN 0: wait at most VTIME tenths of a second
    uint64_t deadline = RX_TIME_INFINITE;
    if (!(__tty_lflag & 2) && __tty_vmin == 0)
        deadline = rx_clock_get_monotonic() + (uint64_t)__tty_vtime * 100000000ull;
    if (f->flags & 04000) deadline = 0; // O_NONBLOCK
    while (f->rpos >= f->rlen) {
        if (!f->rbuf && !(f->rbuf = malloc(4 + RX_CONSOLE_MAX_DATA))) {
            errno = ENOMEM;
            return -1;
        }
        uint32_t nb = 0, observed = 0;
        if (rx_object_wait_one(f->handle, RX_CHANNEL_READABLE | RX_PEER_CLOSED, deadline, &observed) == RX_ERR_TIMED_OUT) {
            if (f->flags & 04000) { errno = EAGAIN; return -1; }
            return 0;
        }
        rx_status_t r = rx_channel_read(f->handle, 0, f->rbuf, 0, 4 + RX_CONSOLE_MAX_DATA, 0, &nb, 0);
        if (r == RX_ERR_SHOULD_WAIT) continue;           // another reader got it first
        if (r == RX_ERR_PEER_CLOSED) return 0;          // console gone: end of input
        if (r != RX_OK) return __set_errno_status(r);
        uint32_t op = 0;
        if (nb >= 4) memcpy(&op, f->rbuf, 4);
        if (op != RX_CONSOLE_OP_INPUT) continue;
        if (nb == 4) return 0;                          // end of input (Ctrl+D)
        f->rpos = 4;
        f->rlen = nb;
    }
    size_t k = f->rlen - f->rpos;
    if (k > n) k = n;
    memcpy(p, f->rbuf + f->rpos, k);
    f->rpos += k;
    return (ssize_t)k;
}

static ssize_t file_read(struct fd *f, char *p, size_t n) {
    uint32_t count = n > RX_FS_MAX_IO ? RX_FS_MAX_IO : (uint32_t)n;
    size_t got = 0;
    int r = __fs_call(f->handle, RX_FS_OP_READ, &count, sizeof count, 0, 0, 0, p, count, &got);
    if (r < 0) { errno = -r; return -1; }
    return (ssize_t)got;
}

static ssize_t file_write(struct fd *f, const char *p, size_t n) {
    size_t done = 0;
    while (done < n) {
        size_t k = n - done > RX_FS_MAX_IO ? RX_FS_MAX_IO : n - done;
        uint64_t wrote = 0;
        int r = __fs_call(f->handle, RX_FS_OP_WRITE, p + done, k, 0, 0, 0, &wrote, sizeof wrote, 0);
        if (r < 0) {
            if (done) break;
            errno = -r;
            return -1;
        }
        done += wrote;
        if (wrote < k) break; // out of space
    }
    return (ssize_t)done;
}

static ssize_t pipe_read(struct fd *f, char *p, size_t n) {
    if (n == 0) return 0;
    for (;;) {
        uint64_t got = 0;
        rx_status_t r = rx_socket_read(f->handle, 0, p, n, &got);
        if (r == RX_OK) return (ssize_t)got;
        if (r == RX_ERR_PEER_CLOSED) return 0; // writer gone, nothing left
        if (r != RX_ERR_SHOULD_WAIT) return __set_errno_status(r);
        rx_object_wait_one(f->handle, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
    }
}

static ssize_t pipe_write(struct fd *f, const char *p, size_t n) {
    size_t done = 0;
    while (done < n) {
        uint64_t put = 0;
        rx_status_t r = rx_socket_write(f->handle, 0, p + done, n - done, &put);
        if (r == RX_OK) {
            done += put;
            continue;
        }
        if (r == RX_ERR_SHOULD_WAIT) {
            rx_object_wait_one(f->handle, RX_CHANNEL_WRITABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
            continue;
        }
        if (done) break;
        if (r == RX_ERR_PEER_CLOSED) {
            errno = EPIPE; // the reader is gone (no SIGPIPE: the writer sees this error)
            return -1;
        }
        return __set_errno_status(r);
    }
    return (ssize_t)done;
}

ssize_t read(int fd, void *buf, size_t n) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    switch (f->kind) {
    case RX_FD_CONSOLE: return console_read(f, buf, n);
    case RX_FD_FILE: return file_read(f, buf, n);
    case RX_FD_PIPE: return pipe_read(f, buf, n);
    default: errno = EBADF; return -1;
    }
}

off_t lseek(int fd, off_t offset, int whence) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (f->kind != RX_FD_FILE) {
        errno = ESPIPE;
        return -1;
    }
    struct { int64_t off; uint32_t whence; uint32_t pad; } req = { offset, (uint32_t)whence, 0 };
    uint64_t pos = 0;
    int r = __fs_call(f->handle, RX_FS_OP_SEEK, &req, sizeof req, 0, 0, 0, &pos, sizeof pos, 0);
    if (r < 0) { errno = -r; return -1; }
    return (off_t)pos;
}

int pipe(int fds[2]) {
    int a = __fd_alloc(0);
    if (a < 0) return -1;
    int b = __fd_alloc(a + 1);
    if (b < 0) return -1;
    rx_handle_t r, w;
    rx_status_t s = rx_socket_create(0, &r, &w);
    if (s != RX_OK) return __set_errno_status(s);
    __fds[a].kind = RX_FD_PIPE;
    __fds[a].handle = r;
    __fds[b].kind = RX_FD_PIPE;
    __fds[b].handle = w;
    fds[0] = a;
    fds[1] = b;
    return 0;
}

ssize_t write(int fd, const void *buf, size_t n) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    switch (f->kind) {
    case RX_FD_CONSOLE:
        return console_write(f->handle, buf, n);
    case RX_FD_FILE:
        return file_write(f, buf, n);
    case RX_FD_PIPE:
        return pipe_write(f, buf, n);
    case RX_FD_LOG: {
        size_t done = 0;
        while (done < n) {
            size_t k = n - done > 4096 ? 4096 : n - done;
            rx_status_t r = rx_debuglog_write(f->handle, 0, (const char *)buf + done, k);
            if (r != RX_OK) return done ? (ssize_t)done : __set_errno_status(r);
            done += k;
        }
        return (ssize_t)n;
    }
    default:
        errno = EBADF;
        return -1;
    }
}

int close(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    release(f);
    return 0;
}

int dup2(int fd, int fd2) {
    struct fd *f = __fd_get(fd);
    if (!f) return -1;
    if (fd2 < 0 || fd2 >= MAX_FDS) {
        errno = EBADF;
        return -1;
    }
    if (fd == fd2) return fd2;
    rx_handle_t h = 0;
    rx_status_t r = rx_handle_duplicate(f->handle, RX_RIGHT_SAME_RIGHTS, &h);
    if (r != RX_OK) return __set_errno_status(r);
    int kind = f->kind, flags = f->flags;
    char *path = f->path ? strdup(f->path) : 0;
    release(&__fds[fd2]);
    __fds[fd2].kind = kind;
    __fds[fd2].handle = h;
    __fds[fd2].flags = flags;
    __fds[fd2].path = path;
    return fd2;
}

int dup(int fd) {
    if (!__fd_get(fd)) return -1;
    for (int i = 0; i < MAX_FDS; i++) {
        if (__fds[i].kind == RX_FD_NONE) return dup2(fd, i);
    }
    errno = EMFILE;
    return -1;
}

int isatty(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return 0;
    errno = ENOTTY;
    if (f->kind == RX_FD_CONSOLE || f->kind == RX_FD_LOG) return 1;
    errno = ENOTTY;
    return 0;
}

void _exit(int status) {
    rx_exit(status);
    __builtin_unreachable();
}

int usleep(unsigned long usec) {
    rx_handle_t t;
    if (rx_timer_create(0, 0, &t) != RX_OK) return -1;
    rx_timer_set(t, rx_clock_get_monotonic() + (uint64_t)usec * 1000, 0);
    rx_object_wait_one(t, RX_TIMER_SIGNALED, RX_TIME_INFINITE, 0);
    rx_handle_close(t);
    return 0;
}

unsigned int sleep(unsigned int seconds) {
    usleep((unsigned long)seconds * 1000000ul);
    return 0;
}
