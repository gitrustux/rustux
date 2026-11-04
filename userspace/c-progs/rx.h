// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Helpers for the kernel test programs: the system calls plus tiny output
// functions that write straight to stdout (unbuffered, so they are safe in
// the tests' extra threads and child processes).

#ifndef RX_H
#define RX_H

#include <rustux/process.h>
#include <rustux/syscalls.h>
#include <unistd.h>

static inline uint64_t rx_strlen(const char *s) { uint64_t n = 0; while (s[n]) n++; return n; }
static inline void rx_puts(const char *s) { write(1, s, rx_strlen(s)); }
static inline void rx_putd(int64_t v) {
    char buf[24];
    int i = 23;
    int neg = v < 0;
    uint64_t u = neg ? (uint64_t)(-v) : (uint64_t)v;
    buf[i] = 0;
    do { buf[--i] = '0' + (u % 10); u /= 10; } while (u);
    if (neg) buf[--i] = '-';
    rx_puts(&buf[i]);
}

#endif
