// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Program entry point. The kernel starts the first thread at _start with
// the bootstrap channel in RDI and the stack as if _start had been called.

#include <rustux/syscalls.h>

void __libc_start(rx_handle_t bootstrap) __attribute__((noreturn));

void _start(rx_handle_t bootstrap) {
    __libc_start(bootstrap);
}
