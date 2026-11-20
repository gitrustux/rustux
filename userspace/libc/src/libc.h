// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Internal declarations shared by the C library's source files.

#ifndef LIBC_INTERNAL_H
#define LIBC_INTERNAL_H

#include <rustux/process.h>
#include <stddef.h>
#include <stdint.h>

#define MAX_FDS 64

struct fd {
    int kind;             // RX_FD_*
    rx_handle_t handle;
    int flags;            // open flags (O_ACCMODE, O_APPEND, O_NONBLOCK)
    int cloexec;          // FD_CLOEXEC (recorded; spawned programs get only fds 0-2)
    char *path;           // absolute path of a file or directory (for openat & co.)
    // Console input left over from the last message
    char *rbuf;
    uint32_t rlen, rpos;
    struct pty *pty;      // a pseudo-terminal's master (pty.c)
    struct sock *sock;    // a network socket (net.c); handle: its data socket once connected
};

// Network sockets (net.c)
struct sock;
void __sock_ref(struct fd *f);
void __sock_release(struct fd *f);
long __sock_read(struct fd *f, char *buf, size_t n);
long __sock_write(struct fd *f, const char *buf, size_t n);
// poll: the events ready now; or the handle and signals to wait for
short __sock_poll_prepare(struct fd *f, short events, rx_handle_t *h, uint32_t *signals);
short __sock_poll_result(struct fd *f, short events, uint32_t pending);

// Pseudo-terminal masters (pty.c)
struct pty;
int __pty_open_slave(const char *path);
long __pty_read(struct fd *f, char *buf, size_t n);
long __pty_write(struct fd *f, const char *buf, size_t n);
uint32_t __pty_poll_signals(void);
void __pty_ref(struct fd *f);
void __pty_release(struct fd *f);

// Threads. The thread pointer (on x86-64 the FS base) points at the
// thread's control block; its thread-local storage lies just below it
// (x86-64 variant II; another architecture's port settles its own:
// docs/PORTING.md).
#define RX_KEYS_MAX 128
struct __pthread {
    struct __pthread *self;     // must stay first: %fs:0 on x86-64
    int err;                    // errno
    volatile int detach_state;  // 0 joinable, 1 detached, 2 joined
    rx_handle_t handle;         // the thread
    rx_handle_t wake;           // event for futex waits (created when first needed)
    uint64_t tid;               // the thread's koid
    void *(*fn)(void *);
    void *arg, *result;
    void *map;                  // stack + TLS + this block (0: main thread)
    size_t map_size;
    char *stack;                // lowest address
    size_t stack_size;
    const void *tsd[RX_KEYS_MAX];
    struct __pthread *next;     // on the list of ended detached threads
    char name[32];
};

static inline struct __pthread *__self(void) { return (struct __pthread *)rx_arch_thread_pointer(); }

// A spin lock for the library's internals (malloc, filesystem calls,
// futex queues): spins briefly, then yields the CPU
void __lock(volatile int *l);
void __unlock(volatile int *l);
// Futexes, emulated in the library: wait while *addr == val (deadline in
// monotonic ns, RX_TIME_INFINITE for none); returns 0, -EAGAIN or -ETIMEDOUT
int __futex_wait(volatile int *addr, int val, uint64_t deadline);
int __futex_wake(volatile int *addr, int n);
void __init_tls(void);
// A boot-image file's bytes as a VMO (RX_FS_OP_GET_VMO); 0 or -errno
int __fs_get_vmo(int fd, rx_handle_t *vmo, uint64_t *offset, uint64_t *size);
void __run_init_array(int argc, char **argv, char **envp);
// Monotonic deadline for an absolute time on `clock` (CLOCK_REALTIME or
// CLOCK_MONOTONIC); RX_TIME_INFINITE for NULL
uint64_t __deadline_abs(int clock, const void *ts);
// The console's terminal modes (termios), for console reads
extern unsigned __tty_lflag, __tty_vmin, __tty_vtime;

extern struct fd __fds[MAX_FDS];
extern char **environ;

void __stdio_init(void);
void __fs_init(void);
// Lowest free fd at or above `from`, or -1 (EMFILE)
int __fd_alloc(int from);
struct fd *__fd_get(int fd);
int __at_path(int dirfd, const char *path, char *out);  // dirfd + path, for the *at calls
// A handle another process can use for `fd` (a new connection for a file,
// a copy otherwise); 0 on failure
rx_handle_t __fd_export(int fd);
// Send a request on a filesystem connection and wait for the reply. `a` and
// `b` are appended after the header; `h` (if not 0) travels with it.
// Returns the reply status (>= 0) or a negative errno; the reply's payload
// goes to `out` (up to `cap` bytes) and its length to *len.
int __fs_call(rx_handle_t ch, uint32_t op, const void *a, size_t alen, const void *b, size_t blen,
              rx_handle_t h, void *out, size_t cap, size_t *len);
void __stdio_flush_all(void);
void __malloc_init(void);
void __env_init(char **envp);
int __set_errno_status(rx_status_t s);   // sets errno, returns -1

#endif
