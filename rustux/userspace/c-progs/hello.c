// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Hello World userspace program for Rustux
//!
//! This simple program demonstrates:
//! - Using sys_write to print to stdout
//! - Using sys_getpid to get the process ID
//! - Using sys_exit to terminate cleanly

#include "syscall.h"

// Userspace entry point
void _start(void) {
    const char *msg1 = "Hello from userspace!\n";
    const char *msg2 = "My PID is: ";
    const char *newline = "\n";

    // Print greeting
    sys_write(STDOUT_FILENO, msg1, 20);

    // Get and print PID
    int64_t pid = sys_getpid();

    // Print PID message
    sys_write(STDOUT_FILENO, msg2, 11);

    // Convert PID to string and print
    char pid_buf[32];
    int i = 0;
    int64_t n = pid;

    // Handle 0 case
    if (n == 0) {
        pid_buf[i++] = '0';
    } else {
        // Convert to string (reversed)
        while (n > 0) {
            pid_buf[i++] = '0' + (n % 10);
            n /= 10;
        }
        // Reverse the string
        for (int j = 0; j < i / 2; j++) {
            char tmp = pid_buf[j];
            pid_buf[j] = pid_buf[i - 1 - j];
            pid_buf[i - 1 - j] = tmp;
        }
    }

    sys_write(STDOUT_FILENO, pid_buf, i);
    sys_write(STDOUT_FILENO, newline, 1);

    // Exit cleanly
    sys_exit(0);
}
