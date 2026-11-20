// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// x86-64 (<rustux/arch.h>). System calls: int 0x80, the number in rax,
// arguments in rdi, rsi, rdx, r10, r8, r9, the result in rax; rcx and
// r11 are clobbered.
#pragma once
#include <stdint.h>

#define RX_ARCH "x86_64"

static inline int64_t rx_syscall(int64_t n, int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, int64_t f) {
    register int64_t r10 __asm__("r10") = d;
    register int64_t r8 __asm__("r8") = e;
    register int64_t r9 __asm__("r9") = f;
    int64_t ret;
    __asm__ volatile("int $0x80"
                     : "=a"(ret)
                     : "a"(n), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8), "r"(r9)
                     : "memory", "rcx", "r11");
    return ret;
}

static inline void rx_cpu_relax(void) { __builtin_ia32_pause(); }
static inline void rx_arch_barrier(void) { __asm__ volatile("mfence" ::: "memory"); }

// The FS base points at the thread's control block, whose first word is
// its own address (variant II)
static inline void *rx_arch_thread_pointer(void) {
    void *t;
    __asm__("mov %%fs:0, %0" : "=r"(t));
    return t;
}

static inline uint64_t rx_arch_cycles(void) {
    uint32_t lo, hi;
    __asm__ volatile("rdtsc" : "=a"(lo), "=d"(hi));
    return (uint64_t)hi << 32 | lo;
}

static inline int rx_arch_hw_random(uint64_t *v) {
    static int known = -1;
    if (known < 0) {
        unsigned a = 1, b, c, d;
        __asm__ volatile("cpuid" : "+a"(a), "=b"(b), "=c"(c), "=d"(d));
        known = (c >> 30) & 1;
    }
    if (!known) return 0;
    unsigned char ok = 0;
    for (int t = 0; t < 10 && !ok; t++) __asm__ volatile("rdrand %0; setc %1" : "=r"(*v), "=qm"(ok));
    return ok;
}

// I/O ports (a driver given them: rx_ioports_request)
#define RX_ARCH_HAS_PORT_IO 1
static inline uint8_t rx_inb(uint16_t port) {
    uint8_t v;
    __asm__ volatile("inb %1, %0" : "=a"(v) : "Nd"(port));
    return v;
}
static inline void rx_outb(uint16_t port, uint8_t v) { __asm__ volatile("outb %0, %1" : : "a"(v), "Nd"(port)); }
static inline uint16_t rx_inw(uint16_t port) {
    uint16_t v;
    __asm__ volatile("inw %1, %0" : "=a"(v) : "Nd"(port));
    return v;
}
static inline void rx_outw(uint16_t port, uint16_t v) { __asm__ volatile("outw %0, %1" : : "a"(v), "Nd"(port)); }
static inline uint32_t rx_inl(uint16_t port) {
    uint32_t v;
    __asm__ volatile("inl %1, %0" : "=a"(v) : "Nd"(port));
    return v;
}
static inline void rx_outl(uint16_t port, uint32_t v) { __asm__ volatile("outl %0, %1" : : "a"(v), "Nd"(port)); }
