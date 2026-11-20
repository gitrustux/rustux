// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// What differs between processors, in one place (docs/PORTING.md): the
// system call instruction, waiting in a spin loop, memory barriers, the
// thread pointer, a cycle counter, the hardware random number. Each
// architecture's header defines:
//
//   RX_ARCH                      "x86_64", "aarch64", "riscv64"
//   rx_syscall(n, a..f)          a system call: the result, or an error
//   rx_cpu_relax()               in a spin loop
//   rx_arch_barrier()            a full memory barrier (also for DMA)
//   rx_arch_thread_pointer()     the thread pointer's value
//   rx_arch_cycles()             a cycle or time counter (for entropy)
//   rx_arch_hw_random(&v)        1 and a random number, or 0
//   RX_ARCH_HAS_PORT_IO          x86 only: rx_inb() and the like exist
//
// Only x86-64 runs today; the others are written to the calling
// convention docs/SYSCALL.md plans for them, so that the library and
// what uses it compile for them (scripts/check-ports.sh).
#pragma once

#if defined(__x86_64__)
#include <rustux/arch/x86_64.h>
#elif defined(__aarch64__)
#include <rustux/arch/aarch64.h>
#elif defined(__riscv) && __riscv_xlen == 64
#include <rustux/arch/riscv64.h>
#else
#error "Rustux has no port for this architecture (docs/PORTING.md)"
#endif
