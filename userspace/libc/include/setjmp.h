// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// setjmp and longjmp (2026-11-26: libpng, libjpeg and curl need them). In
// each architecture's part of the library (src/arch/): the registers a
// function must keep, the stack pointer and where to go back to. There
// are no signal masks to keep, so sigsetjmp is setjmp.

#ifndef _SETJMP_H
#define _SETJMP_H

// Room for every architecture's registers (riscv64: 26; aarch64: 22;
// x86-64: 8)
typedef unsigned long jmp_buf[32];
typedef unsigned long sigjmp_buf[32];

int setjmp(jmp_buf env) __attribute__((returns_twice));
_Noreturn void longjmp(jmp_buf env, int value);
int _setjmp(jmp_buf env) __attribute__((returns_twice));
_Noreturn void _longjmp(jmp_buf env, int value);
int sigsetjmp(sigjmp_buf env, int savemask) __attribute__((returns_twice));
_Noreturn void siglongjmp(sigjmp_buf env, int value);

#endif
