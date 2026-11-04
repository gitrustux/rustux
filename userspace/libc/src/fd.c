// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// File descriptors. Each one is a handle plus a kind that says how to
// talk to it. Step 7 has the console (a channel to the console service)
// and the debug log; files and pipes come in step 8.

#include "libc.h"
#include <errno.h>
#include <rustux/console.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

struct fd __fds[MAX_FDS];

static struct fd *get(int fd) {
    if (fd < 0 || fd >= MAX_FDS || __fds[fd].kind == RX_FD_NONE) {
        errno = EBADF;
        return 0;
    }
    return &__fds[fd];
}

static void release(struct fd *f) {
    if (f->handle) rx_handle_close(f->handle);
    free(f->rbuf);
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
    struct fd *f = get(fd);
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
    while (f->rpos >= f->rlen) {
        if (!f->rbuf && !(f->rbuf = malloc(4 + RX_CONSOLE_MAX_DATA))) {
            errno = ENOMEM;
            return -1;
        }
        uint32_t nb = 0, observed = 0;
        rx_object_wait_one(f->handle, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &observed);
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

ssize_t read(int fd, void *buf, size_t n) {
    struct fd *f = get(fd);
    if (!f) return -1;
    switch (f->kind) {
    case RX_FD_CONSOLE: return console_read(f, buf, n);
    default: errno = EBADF; return -1;
    }
}

ssize_t write(int fd, const void *buf, size_t n) {
    struct fd *f = get(fd);
    if (!f) return -1;
    switch (f->kind) {
    case RX_FD_CONSOLE:
        return console_write(f->handle, buf, n);
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
    struct fd *f = get(fd);
    if (!f) return -1;
    release(f);
    return 0;
}

int dup2(int fd, int fd2) {
    struct fd *f = get(fd);
    if (!f) return -1;
    if (fd2 < 0 || fd2 >= MAX_FDS) {
        errno = EBADF;
        return -1;
    }
    if (fd == fd2) return fd2;
    rx_handle_t h = 0;
    rx_status_t r = rx_handle_duplicate(f->handle, RX_RIGHT_SAME_RIGHTS, &h);
    if (r != RX_OK) return __set_errno_status(r);
    int kind = f->kind;
    release(&__fds[fd2]);
    __fds[fd2].kind = kind;
    __fds[fd2].handle = h;
    return fd2;
}

int dup(int fd) {
    if (!get(fd)) return -1;
    for (int i = 0; i < MAX_FDS; i++) {
        if (__fds[i].kind == RX_FD_NONE) return dup2(fd, i);
    }
    errno = EMFILE;
    return -1;
}

int isatty(int fd) {
    struct fd *f = get(fd);
    if (!f) return 0;
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
