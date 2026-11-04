// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Process startup and spawning in the Rustux C library.

#ifndef RUSTUX_PROCESS_H
#define RUSTUX_PROCESS_H

#include <rustux/syscalls.h>

// Handles from the bootstrap message (0 if not given)
extern rx_handle_t rx_proc_self, rx_thread_self, rx_job_default, rx_vmar_root;

// Take another bootstrap handle by its info word (e.g. RX_PA_USER0);
// 0 if there is none. Each handle can be taken once.
rx_handle_t rx_take_startup_handle(uint32_t info);

// File descriptor kinds
#define RX_FD_NONE    0
#define RX_FD_CONSOLE 1   // a channel to the console service
#define RX_FD_LOG     2   // a debug log handle (output only)

// Put handle `h` (of kind `kind`) behind file descriptor `fd`, closing
// what was there. Returns 0 or -1 (errno set).
int rx_fd_bind(int fd, int kind, rx_handle_t h);
// The handle behind `fd` (0 if none); the fd keeps owning it
rx_handle_t rx_fd_handle(int fd);
// Errno value for a negative rx_status_t
int rx_status_to_errno(rx_status_t s);

// rx_spawn flags
#define RX_SPAWN_CLONE_STDIO 1u   // the child gets copies of fds 0, 1 and 2

typedef struct {
    uint32_t info;       // e.g. RX_PA_USER0 or RX_PA_HND(RX_PA_FD, 3)
    rx_handle_t handle;  // moved into the child
} rx_spawn_handle_t;

// Start the program at `path` (a /bin/... path for now) with `argv`
// (NULL-terminated; NULL means just the path) and `envp` (NULL means
// the caller's environment). `handles` are moved into the child, even on
// failure. Returns 0 and a process handle in *proc_out (wait for
// RX_PROCESS_TERMINATED), or a negative rx_status_t.
rx_status_t rx_spawn(const char *path, char *const argv[], char *const envp[], uint32_t flags,
                     const rx_spawn_handle_t *handles, uint32_t nhandles, rx_handle_t *proc_out);

// Wait for a process to end and return its exit code (closes `proc`)
int64_t rx_wait_process(rx_handle_t proc);

#endif
