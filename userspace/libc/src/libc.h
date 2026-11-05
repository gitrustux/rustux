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
};

// Threads. The thread pointer (FS base) points at the thread's control
// block; its thread-local storage lies just below it (x86-64 variant II).
#define RX_KEYS_MAX 128
struct __pthread {
    struct __pthread *self;     // must stay first: %fs:0
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

static inline struct __pthread *__self(void) {
    struct __pthread *t;
    __asm__("mov %%fs:0, %0" : "=r"(t));
    return t;
}

// A spin lock for the library's internals (malloc, filesystem calls,
// futex queues): spins briefly, then yields the CPU
void __lock(volatile int *l);
void __unlock(volatile int *l);
// Futexes, emulated in the library: wait while *addr == val (deadline in
// monotonic ns, RX_TIME_INFINITE for none); returns 0, -EAGAIN or -ETIMEDOUT
int __futex_wait(volatile int *addr, int val, uint64_t deadline);
int __futex_wake(volatile int *addr, int n);
void __init_tls(void);
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
