// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// AArch64 (<rustux/arch.h>; not running yet: docs/PORTING.md). System
// calls as docs/SYSCALL.md plans them: svc #0, the number in x8,
// arguments in x0-x5, the result in x0.
#pragma once
#include <stdint.h>

#define RX_ARCH "aarch64"

static inline int64_t rx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
    register int64_t x8 __asm__("x8") = n;
    register int64_t x0 __asm__("x0") = a;
    register int64_t x1 __asm__("x1") = b;
    register int64_t x2 __asm__("x2") = c;
    register int64_t x3 __asm__("x3") = d;
    register int64_t x4 __asm__("x4") = e;
    register int64_t x5 __asm__("x5") = f;
    __asm__ volatile("svc #0" : "+r"(x0) : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5) : "memory");
    return x0;
}

static inline void rx_cpu_relax(void) { __asm__ volatile("yield" ::: "memory"); }
static inline void rx_arch_barrier(void) { __asm__ volatile("dmb sy" ::: "memory"); }

// TPIDR_EL0; the control block's layout around it is the port's to
// settle (variant I)
static inline void *rx_arch_thread_pointer(void) {
    void *t;
    __asm__("mrs %0, tpidr_el0" : "=r"(t));
    return t;
}

static inline uint64_t rx_arch_cycles(void) {
    uint64_t v;
    __asm__ volatile("mrs %0, cntvct_el0" : "=r"(v));
    return v;
}

// RNDR needs FEAT_RNG and a way to know it is there: not yet
static inline int rx_arch_hw_random(uint64_t *v) {
    (void)v;
    return 0;
}
