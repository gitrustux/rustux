// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Linux system call numbers that the C library's syscall() emulates for
// programs built for Linux's ABI (Rust's std): futex, getrandom, gettid.
// Others fail with ENOSYS.

#ifndef _SYS_SYSCALL_H
#define _SYS_SYSCALL_H

#define SYS_gettid 186
#define SYS_futex 202
#define SYS_getrandom 318

long syscall(long number, ...);

#endif
