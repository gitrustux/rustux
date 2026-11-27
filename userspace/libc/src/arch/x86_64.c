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

#if defined(__x86_64__)
// setjmp and longjmp (<setjmp.h>): rbx, rbp, r12-r15, the stack pointer
// after the return, the return address
__asm__(".text\n"
        ".globl setjmp\n.globl _setjmp\n.globl sigsetjmp\n"
        "setjmp:\n_setjmp:\nsigsetjmp:\n"
        "    mov %rbx, 0(%rdi)\n"
        "    mov %rbp, 8(%rdi)\n"
        "    mov %r12, 16(%rdi)\n"
        "    mov %r13, 24(%rdi)\n"
        "    mov %r14, 32(%rdi)\n"
        "    mov %r15, 40(%rdi)\n"
        "    lea 8(%rsp), %rdx\n"
        "    mov %rdx, 48(%rdi)\n"
        "    mov (%rsp), %rdx\n"
        "    mov %rdx, 56(%rdi)\n"
        "    xor %eax, %eax\n"
        "    ret\n"
        ".globl longjmp\n.globl _longjmp\n.globl siglongjmp\n"
        "longjmp:\n_longjmp:\nsiglongjmp:\n"
        "    mov %esi, %eax\n"
        "    test %eax, %eax\n"
        "    jnz 1f\n"
        "    inc %eax\n"
        "1:  mov 0(%rdi), %rbx\n"
        "    mov 8(%rdi), %rbp\n"
        "    mov 16(%rdi), %r12\n"
        "    mov 24(%rdi), %r13\n"
        "    mov 32(%rdi), %r14\n"
        "    mov 40(%rdi), %r15\n"
        "    mov 48(%rdi), %rsp\n"
        "    jmp *56(%rdi)\n");
#endif
