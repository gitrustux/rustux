// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// RISC-V 64 (<rustux/arch.h>; not running yet: docs/PORTING.md). System
// calls as docs/SYSCALL.md plans them: ecall, the number in a7,
// arguments in a0-a5, the result in a0.
#pragma once
#include <stdint.h>

#define RX_ARCH "riscv64"

static inline int64_t rx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
    register int64_t a7 __asm__("a7") = n;
    register int64_t a0 __asm__("a0") = a;
    register int64_t a1 __asm__("a1") = b;
    register int64_t a2 __asm__("a2") = c;
    register int64_t a3 __asm__("a3") = d;
    register int64_t a4 __asm__("a4") = e;
    register int64_t a5 __asm__("a5") = f;
    __asm__ volatile("ecall" : "+r"(a0) : "r"(a7), "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5) : "memory");
    return a0;
}

// (pause needs Zihintpause; a plain nop elsewhere is as good)
static inline void rx_cpu_relax(void) { __asm__ volatile("nop" ::: "memory"); }
static inline void rx_arch_barrier(void) { __asm__ volatile("fence iorw, iorw" ::: "memory"); }

// tp; the control block's layout around it is the port's to settle
static inline void *rx_arch_thread_pointer(void) {
    void *t;
    __asm__("mv %0, tp" : "=r"(t));
    return t;
}

static inline uint64_t rx_arch_cycles(void) {
    uint64_t v;
    __asm__ volatile("rdtime %0" : "=r"(v));
    return v;
}

// Zkr's seed CSR is not readable from user mode by default: not yet
static inline int rx_arch_hw_random(uint64_t *v) {
    (void)v;
    return 0;
}
