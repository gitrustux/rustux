// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// RISC-V 64's part of the library (not running yet: docs/PORTING.md):
// where a thread starts, the control block in a0.
#if defined(__riscv) && __riscv_xlen == 64
__asm__(".text\n"
        ".globl __thread_entry\n"
        "__thread_entry:\n"
        "    li fp, 0\n"
        "    li ra, 0\n"
        "    andi sp, sp, -16\n"
        "    call __thread_start\n"
        "    unimp\n");
#endif
