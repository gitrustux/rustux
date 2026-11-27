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

#if defined(__riscv) && __riscv_xlen == 64
// setjmp and longjmp (<setjmp.h>): ra, sp, s0-s11, fs0-fs11 with the D
// extension
#if defined(__riscv_flen) && __riscv_flen >= 64
#define RX_FSAVE(op) \
    "    " op " fs0, 112(a0)\n    " op " fs1, 120(a0)\n    " op " fs2, 128(a0)\n    " op " fs3, 136(a0)\n" \
    "    " op " fs4, 144(a0)\n    " op " fs5, 152(a0)\n    " op " fs6, 160(a0)\n    " op " fs7, 168(a0)\n" \
    "    " op " fs8, 176(a0)\n    " op " fs9, 184(a0)\n    " op " fs10, 192(a0)\n    " op " fs11, 200(a0)\n"
#else
#define RX_FSAVE(op) ""
#endif
__asm__(".text\n"
        ".globl setjmp\n.globl _setjmp\n.globl sigsetjmp\n"
        "setjmp:\n_setjmp:\nsigsetjmp:\n"
        "    sd ra, 0(a0)\n    sd sp, 8(a0)\n"
        "    sd s0, 16(a0)\n    sd s1, 24(a0)\n    sd s2, 32(a0)\n    sd s3, 40(a0)\n"
        "    sd s4, 48(a0)\n    sd s5, 56(a0)\n    sd s6, 64(a0)\n    sd s7, 72(a0)\n"
        "    sd s8, 80(a0)\n    sd s9, 88(a0)\n    sd s10, 96(a0)\n    sd s11, 104(a0)\n"
        RX_FSAVE("fsd")
        "    li a0, 0\n"
        "    ret\n"
        ".globl longjmp\n.globl _longjmp\n.globl siglongjmp\n"
        "longjmp:\n_longjmp:\nsiglongjmp:\n"
        "    ld ra, 0(a0)\n    ld sp, 8(a0)\n"
        "    ld s0, 16(a0)\n    ld s1, 24(a0)\n    ld s2, 32(a0)\n    ld s3, 40(a0)\n"
        "    ld s4, 48(a0)\n    ld s5, 56(a0)\n    ld s6, 64(a0)\n    ld s7, 72(a0)\n"
        "    ld s8, 80(a0)\n    ld s9, 88(a0)\n    ld s10, 96(a0)\n    ld s11, 104(a0)\n"
        RX_FSAVE("fld")
        "    seqz a0, a1\n"
        "    add a0, a0, a1\n"
        "    ret\n");
#endif
