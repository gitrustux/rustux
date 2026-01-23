// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Counter userspace program for Rustux
//!
//! This program demonstrates:
//! - Loops and counting
//! - Using sys_write for output
//! - Using sys_getpid to identify the process
//! - Using sys_yield to give up CPU time
//! - Using sys_exit to terminate

#include "syscall.h"

// Userspace entry point
void _start(void) {
    const char *msg1 = "Counter PID: ";
    const char *msg2 = " count: ";
    const char *newline = "\n";

    // Get our PID
    int64_t pid = sys_getpid();

    // Count from 0 to 99
    for (int i = 0; i < 100; i++) {
        // Print "Counter PID: X count: Y\n"
        sys_write(STDOUT_FILENO, msg1, 13);

        // Print PID
        char pid_buf[32];
        int j = 0;
        int64_t n = pid;
        if (n == 0) {
            pid_buf[j++] = '0';
        } else {
            while (n > 0) {
                pid_buf[j++] = '0' + (n % 10);
                n /= 10;
            }
            // Reverse
            for (int k = 0; k < j / 2; k++) {
                char tmp = pid_buf[k];
                pid_buf[k] = pid_buf[j - 1 - k];
                pid_buf[j - 1 - k] = tmp;
            }
        }
        sys_write(STDOUT_FILENO, pid_buf, j);

        // Print " count: "
        sys_write(STDOUT_FILENO, msg2, 8);

        // Print count
        char count_buf[32];
        j = 0;
        n = i;
        if (n == 0) {
            count_buf[j++] = '0';
        } else {
            while (n > 0) {
                count_buf[j++] = '0' + (n % 10);
                n /= 10;
            }
            // Reverse
            for (int k = 0; k < j / 2; k++) {
                char tmp = count_buf[k];
                count_buf[k] = count_buf[j - 1 - k];
                count_buf[j - 1 - k] = tmp;
            }
        }
        sys_write(STDOUT_FILENO, count_buf, j);

        sys_write(STDOUT_FILENO, newline, 1);

        // Yield CPU to other processes
        sys_yield();
    }

    // Exit cleanly
    sys_exit(0);
}
