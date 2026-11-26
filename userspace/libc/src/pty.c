// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Pseudo-terminals (DESKTOP.md step F7). posix_openpt makes a console
// channel (<rustux/console.h>) and gives this process its console end as
// the master: writing to the master types (through the line discipline,
// <rustux/ldisc.h>, run here), reading it gives what the programs print
// and the echo. The other end is the slave, the same kind of handle as
// the text console's: open(ptsname(master)) once in this process gives
// it as a file descriptor to pass to a program (posix_spawn, dup2). When
// every copy of the slave is closed, reading the master fails with EIO.
// ioctl(master, TIOCSWINSZ) sets the size that TIOCGWINSZ reports to the
// programs.
//
// There is no /dev/pts directory: a slave has a name only in the process
// that opened its master.

#include "libc.h"
#include <errno.h>
#include <fcntl.h>
#include <rustux/ldisc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// Raised on the master's end while echo is waiting to be read
#define PTY_SIGNAL_ECHO (RX_USER_SIGNAL_0 << 1)

struct pty {
    int refs;              // file descriptors sharing it (dup)
    volatile int lock;
    rx_ldisc_t ld;         // ld.ch is the master's end
    rx_handle_t slave;     // the slave's end until it is opened
    int num;
    char *out;             // echo, and output read but not yet taken
    size_t out_len, out_cap;
    char msg[4 + RX_CONSOLE_MAX_DATA];
};

static int next_num;

static void append(struct pty *p, const char *s, size_t n) {
    if (p->out_len + n > p->out_cap) {
        size_t cap = p->out_cap ? p->out_cap * 2 : 1024;
        while (cap < p->out_len + n) cap *= 2;
        if (cap > (1u << 20)) return; // nobody reads: drop it
        char *o = realloc(p->out, cap);
        if (!o) return;
        p->out = o;
        p->out_cap = cap;
    }
    memcpy(p->out + p->out_len, s, n);
    p->out_len += n;
    rx_object_signal(p->ld.ch, 0, PTY_SIGNAL_ECHO);
}

static void pty_echo(rx_ldisc_t *ld, const char *s, size_t n) {
    append(ld->ctx, s, n);
}

int posix_openpt(int flags) {
    (void)flags;
    int fd = __fd_alloc(0);
    if (fd < 0) return -1;
    struct pty *p = calloc(1, sizeof *p);
    if (!p) {
        __fd_unreserve(fd);
        errno = ENOMEM;
        return -1;
    }
    rx_handle_t master, slave;
    if (rx_channel_create(0, &master, &slave) != RX_OK) {
        free(p);
        __fd_unreserve(fd);
        errno = ENOMEM;
        return -1;
    }
    // The line discipline keeps a handle of its own (dup'd descriptors
    // may be closed in any order)
    rx_handle_t own;
    if (rx_handle_duplicate(master, RX_RIGHT_SAME_RIGHTS, &own) != RX_OK) {
        rx_handle_close(master);
        rx_handle_close(slave);
        free(p);
        __fd_unreserve(fd);
        errno = ENOMEM;
        return -1;
    }
    p->refs = 1;
    p->slave = slave;
    p->num = next_num++;
    rx_ldisc_init(&p->ld, own, pty_echo, p);
    rx_tty_set_size(master, 80, 24);
    __fds[fd].handle = master;
    __fds[fd].flags = O_RDWR;
    __fds[fd].pty = p;
    __atomic_store_n(&__fds[fd].kind, RX_FD_PTY, __ATOMIC_RELEASE);
    return fd;
}

static struct pty *master(int fd) {
    struct fd *f = __fd_get(fd);
    if (!f) return 0;
    if (f->kind != RX_FD_PTY) {
        errno = EINVAL;
        return 0;
    }
    return f->pty;
}

int grantpt(int fd) { return master(fd) ? 0 : -1; }
int unlockpt(int fd) { return master(fd) ? 0 : -1; }

int ptsname_r(int fd, char *buf, size_t size) {
    struct pty *p = master(fd);
    if (!p) return ENOTTY;
    if ((size_t)snprintf(buf, size, "/dev/pts/%d", p->num) >= size) return ERANGE;
    return 0;
}

char *ptsname(int fd) {
    static char buf[32];
    return ptsname_r(fd, buf, sizeof buf) ? 0 : buf;
}

// open("/dev/pts/N"): the slave of a master in this process, once
int __pty_open_slave(const char *path) {
    int num = atoi(path + 9);
    for (int i = 0; i < MAX_FDS; i++) {
        struct pty *p = __fds[i].kind == RX_FD_PTY ? __fds[i].pty : 0;
        if (!p || p->num != num) continue;
        __lock(&p->lock);
        rx_handle_t h = p->slave;
        p->slave = 0;
        __unlock(&p->lock);
        if (!h) {
            errno = EIO; // already opened
            return -1;
        }
        int fd = __fd_alloc(0);
        if (fd < 0) {
            p->slave = h;
            return -1;
        }
        __fds[fd].handle = h;
        __fds[fd].flags = O_RDWR;
        __atomic_store_n(&__fds[fd].kind, RX_FD_CONSOLE, __ATOMIC_RELEASE);
        return fd;
    }
    errno = ENOENT;
    return -1;
}

ssize_t __pty_read(struct fd *f, char *buf, size_t n) {
    struct pty *p = f->pty;
    char *msg = p->msg;
    if (!n) return 0;
    for (;;) {
        __lock(&p->lock);
        if (p->out_len) {
            size_t k = p->out_len < n ? p->out_len : n;
            memcpy(buf, p->out, k);
            memmove(p->out, p->out + k, p->out_len - k);
            p->out_len -= k;
            if (!p->out_len) rx_object_signal(p->ld.ch, PTY_SIGNAL_ECHO, 0);
            __unlock(&p->lock);
            return (ssize_t)k;
        }
        uint32_t nb = 0;
        rx_status_t r = rx_channel_read(p->ld.ch, 0, msg, 0, sizeof p->msg, 0, &nb, 0);
        if (r == RX_OK) {
            const char *data;
            size_t len;
            if (rx_ldisc_message(&p->ld, msg, nb, &data, &len) == 1 && len) {
                size_t k = len < n ? len : n;
                memcpy(buf, data, k);
                if (k < len) append(p, data + k, len - k);
                __unlock(&p->lock);
                return (ssize_t)k;
            }
            __unlock(&p->lock);
            continue;
        }
        __unlock(&p->lock);
        if (r == RX_ERR_PEER_CLOSED) {
            errno = EIO; // every slave is closed
            return -1;
        }
        if (r != RX_ERR_SHOULD_WAIT) return __set_errno_status(r);
        if (f->flags & O_NONBLOCK) {
            errno = EAGAIN;
            return -1;
        }
        rx_object_wait_one(p->ld.ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED | PTY_SIGNAL_ECHO, RX_TIME_INFINITE, 0);
    }
}

ssize_t __pty_write(struct fd *f, const char *buf, size_t n) {
    struct pty *p = f->pty;
    __lock(&p->lock);
    rx_ldisc_input(&p->ld, buf, n);
    __unlock(&p->lock);
    return (ssize_t)n;
}

// Signals that make the master readable (poll)
uint32_t __pty_poll_signals(void) { return RX_CHANNEL_READABLE | RX_PEER_CLOSED | PTY_SIGNAL_ECHO; }

void __pty_ref(struct fd *f) {
    struct pty *p = f->pty;
    __lock(&p->lock);
    p->refs++;
    __unlock(&p->lock);
}

void __pty_release(struct fd *f) {
    struct pty *p = f->pty;
    f->pty = 0;
    if (!p) return;
    __lock(&p->lock);
    int last = --p->refs == 0;
    __unlock(&p->lock);
    if (!last) return;
    if (p->slave) rx_handle_close(p->slave);
    rx_handle_close(p->ld.ch);
    free(p->out);
    free(p);
    // f->handle (the master's end) is closed by the caller
}
