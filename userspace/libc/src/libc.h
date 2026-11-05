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
    // Console input left over from the last message
    char *rbuf;
    uint32_t rlen, rpos;
};

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
