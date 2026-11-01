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
#define SYS_YIELD        0x72
#define SYS_SPAWN_PROCESS 0x0B
#define SYS_WAIT_ONE     0x26
#define SYS_HANDLE_CLOSE 0x07
#define SYS_GET_INFO     0x0A

#define PROCESS_TERMINATED (1u << 3)
#define INFO_PROCESS       3

// File descriptor numbers
#define STDOUT  1

static inline long syscall1(long number, long arg1) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (arg1)
        : "memory"
    );
    return ret;
}

static inline long syscall3(long number, long arg1, long arg2, long arg3) {
    long ret;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (arg1), "S" (arg2), "d" (arg3)
        : "memory"
    );
    return ret;
}

static inline long syscall4(long number, long arg1, long arg2, long arg3, long arg4) {
    long ret;
    register long r10 __asm__("r10") = arg4;
    __asm__ volatile (
        "int $0x80"
        : "=a" (ret)
        : "a" (number), "D" (arg1), "S" (arg2), "d" (arg3), "r" (r10)
        : "memory"
    );
    return ret;
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
    syscall3(SYS_WRITE, STDOUT, (long)str, len);
}

void _start(void) {
    // No screen clear: keep the kernel boot log (USB, PS/2) visible
    print("Init process (PID 1) starting...\n");
    print("Spawning shell...\n\n");

    // Run the shell; if it ever exits, start a new one
    for (;;) {
        unsigned int proc = 0;
        long result = syscall3(SYS_SPAWN_PROCESS, (long)"/bin/shell", (long)&proc, 0);
        if (result < 0) {
            print("Failed to spawn shell!\n");
            sys_exit(1);
        }
        // Sleep until the shell's process is TERMINATED
        syscall4(SYS_WAIT_ONE, proc, PROCESS_TERMINATED, -1L, 0);
        syscall1(SYS_HANDLE_CLOSE, proc);
        print("\nShell exited; starting a new one\n\n");
    }
}
