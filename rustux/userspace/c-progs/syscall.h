// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Syscall interface for Rustux userspace programs
//!
//! This header provides inline functions for making syscalls
//! to the Rustux kernel from userspace C programs.

#ifndef SYSCALL_H
#define SYSCALL_H

#include <stdint.h>

// Syscall numbers
#define SYS_PROCESS_CREATE  0x01
#define SYS_PROCESS_EXIT    0x06
#define SYS_CLOCK_GET       0x40
#define SYS_DEBUG_WRITE     0x50
#define SYS_WRITE           0x60
#define SYS_READ            0x61
#define SYS_OPEN            0x62
#define SYS_CLOSE           0x63
#define SYS_LSEEK           0x64
#define SYS_GETPID          0x70
#define SYS_GETPPID         0x71
#define SYS_YIELD           0x72

// Open flags
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR   2

// Seek whence
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2

// File descriptors
#define STDIN_FILENO  0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

/**
 * Make a syscall with 0 arguments
 */
static inline int64_t syscall0(int num) {
    int64_t ret;
    __asm__ volatile (
        "mov %0, %%rax\n"
        "int $0x80\n"
        "mov %%rax, %0\n"
        : "=r" (ret)
        : "0" ((int64_t)num)
        : "rax", "rcx", "r11"
    );
    return ret;
}

/**
 * Make a syscall with 1 argument
 */
static inline int64_t syscall1(int num, int64_t arg1) {
    int64_t ret;
    __asm__ volatile (
        "mov %1, %%rax\n"
        "mov %2, %%rdi\n"
        "int $0x80\n"
        "mov %%rax, %0\n"
        : "=r" (ret)
        : "r" ((int64_t)num), "r" (arg1)
        : "rax", "rdi", "rcx", "r11"
    );
    return ret;
}

/**
 * Make a syscall with 2 arguments
 */
static inline int64_t syscall2(int num, int64_t arg1, int64_t arg2) {
    int64_t ret;
    __asm__ volatile (
        "mov %1, %%rax\n"
        "mov %2, %%rdi\n"
        "mov %3, %%rsi\n"
        "int $0x80\n"
        "mov %%rax, %0\n"
        : "=r" (ret)
        : "r" ((int64_t)num), "r" (arg1), "r" (arg2)
        : "rax", "rdi", "rsi", "rcx", "r11"
    );
    return ret;
}

/**
 * Make a syscall with 3 arguments
 */
static inline int64_t syscall3(int num, int64_t arg1, int64_t arg2, int64_t arg3) {
    int64_t ret;
    __asm__ volatile (
        "mov %1, %%rax\n"
        "mov %2, %%rdi\n"
        "mov %3, %%rsi\n"
        "mov %4, %%rdx\n"
        "int $0x80\n"
        "mov %%rax, %0\n"
        : "=r" (ret)
        : "r" ((int64_t)num), "r" (arg1), "r" (arg2), "r" (arg3)
        : "rax", "rdi", "rsi", "rdx", "rcx", "r11"
    );
    return ret;
}

// Helper functions

/**
 * Write to a file descriptor
 */
static inline int64_t sys_write(int fd, const void *buf, int64_t len) {
    return syscall3(SYS_WRITE, (int64_t)fd, (int64_t)buf, len);
}

/**
 * Read from a file descriptor
 */
static inline int64_t sys_read(int fd, void *buf, int64_t len) {
    return syscall3(SYS_READ, (int64_t)fd, (int64_t)buf, len);
}

/**
 * Open a file
 */
static inline int64_t sys_open(const char *path, int flags) {
    return syscall2(SYS_OPEN, (int64_t)path, (int64_t)flags);
}

/**
 * Close a file descriptor
 */
static inline int64_t sys_close(int fd) {
    return syscall1(SYS_CLOSE, (int64_t)fd);
}

/**
 * Seek in a file
 */
static inline int64_t sys_lseek(int fd, int64_t offset, int whence) {
    return syscall3(SYS_LSEEK, (int64_t)fd, offset, (int64_t)whence);
}

/**
 * Get current process ID
 */
static inline int64_t sys_getpid(void) {
    return syscall0(SYS_GETPID);
}

/**
 * Get parent process ID
 */
static inline int64_t sys_getppid(void) {
    return syscall0(SYS_GETPPID);
}

/**
 * Yield CPU to scheduler
 */
static inline int64_t sys_yield(void) {
    return syscall0(SYS_YIELD);
}

/**
 * Exit the current process
 */
static inline void sys_exit(int code) __attribute__((noreturn));
static inline void sys_exit(int code) {
    (void)syscall1(SYS_PROCESS_EXIT, (int64_t)code);
    for (;;) { __asm__ volatile("hlt"); }
}

/**
 * Debug write (to port 0xE9)
 */
static inline int64_t sys_debug_write(const void *buf, int64_t len) {
    return syscall2(SYS_DEBUG_WRITE, (int64_t)buf, len);
}

#endif // SYSCALL_H
