// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Init Process (PID 1)
//!
//! This is the first userspace process started by the kernel.
//! Its job is to initialize the system and launch the shell.
//!
//! Build: x86_64-linux-gnu-gcc -static -nostdlib -fno-stack-protector init.c -o init.elf

// Syscall numbers
#define SYS_WRITE        0x60
#define SYS_SPAWN        0x03
#define SYS_EXIT         0x06

// File descriptor numbers
#define STDOUT  1

static inline long syscall1(long number, long arg1) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "b" (arg1)
        : "memory"
    );
    return ret;
}

static inline long sys_spawn(const char *path) {
    return syscall1(SYS_SPAWN, (long)path);
}

static inline void sys_exit(int code) {
    syscall1(SYS_EXIT, code);
    __builtin_unreachable();
}

static inline void print(const char *str) {
    long len = 0;
    const char *p = str;
    while (*p) {
        len++;
        p++;
    }
    syscall1(SYS_WRITE, (long)str);
    syscall1(SYS_WRITE, len);
}

void _start(void) {
    print("\033[2J\033[H");  // Clear screen
    print("Init process (PID 1) starting...\n");
    print("Spawning shell...\n\n");

    // Spawn the shell
    long result = sys_spawn("/bin/shell");

    if (result < 0) {
        print("Failed to spawn shell!\n");
        sys_exit(1);
    }

    // If shell exits, we exit too
    print("Shell exited, shutting down...\n");
    sys_exit(0);
}
