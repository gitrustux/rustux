// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// x86-64's part of the library (docs/PORTING.md): where a thread starts.
// The kernel starts it with the control block in rdi and RSP 16-byte
// aligned; set up a frame as if __thread_start had been called.
#if defined(__x86_64__)
__asm__(".text\n"
        ".globl __thread_entry\n"
        "__thread_entry:\n"
        "    xor %ebp, %ebp\n"
        "    and $-16, %rsp\n"
        "    call __thread_start\n"
        "    ud2\n");
#endif
