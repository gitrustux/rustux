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
void __stdio_flush_all(void);
void __malloc_init(void);
void __env_init(char **envp);
int __set_errno_status(rx_status_t s);   // sets errno, returns -1

#endif
