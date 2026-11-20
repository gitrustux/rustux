// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The system call as a function (docs/PORTING.md): what Rust programs
// call (extern "C" __rx_syscall), so that the instruction and its
// registers are written once for each architecture, in <rustux/arch.h>,
// and no program has its own.
#include <rustux/syscalls.h>

int64_t __rx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
    return rx_syscall(n, a, b, c, d, e, f);
}
