// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// AArch64's part of the library (not running yet: docs/PORTING.md):
// where a thread starts, the control block in x0.
#if defined(__aarch64__)
__asm__(".text\n"
        ".globl __thread_entry\n"
        "__thread_entry:\n"
        "    mov x29, #0\n"
        "    mov x30, #0\n"
        "    mov x9, sp\n"
        "    and x9, x9, #-16\n"
        "    mov sp, x9\n"
        "    bl __thread_start\n"
        "    brk #0\n");
#endif
