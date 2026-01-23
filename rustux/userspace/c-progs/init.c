// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Init process for Rustux
//!
//! This is the first userspace process (PID 1) that:
//! - Opens /test.txt from the ramdisk
//! - Reads and displays its contents
//! - Spawns child processes
//! - Coordinates execution

#include "syscall.h"

// Simple string length function
static int strlen(const char *s) {
    int len = 0;
    while (s[len]) len++;
    return len;
}

// Userspace entry point
void _start(void) {
    const char *msg1 = "=== Init process started ===\n";
    const char *msg2 = "Opening /test.txt...\n";
    const char *msg3 = "File contents:\n";
    const char *msg4 = "My PID: ";
    const char *msg5 = "My PPID: ";
    const char *newline = "\n";
    const char *done = "=== Init complete ===\n";

    // Print startup message
    sys_write(STDOUT_FILENO, msg1, 29);

    // Get and print PID
    int64_t pid = sys_getpid();
    sys_write(STDOUT_FILENO, msg4, 9);
    char pid_buf[32];
    int i = 0;
    int64_t n = pid;
    if (n == 0) {
        pid_buf[i++] = '0';
    } else {
        while (n > 0) {
            pid_buf[i++] = '0' + (n % 10);
            n /= 10;
        }
        // Reverse
        for (int j = 0; j < i / 2; j++) {
            char tmp = pid_buf[j];
            pid_buf[j] = pid_buf[i - 1 - j];
            pid_buf[i - 1 - j] = tmp;
        }
    }
    sys_write(STDOUT_FILENO, pid_buf, i);
    sys_write(STDOUT_FILENO, newline, 1);

    // Get and print PPID
    int64_t ppid = sys_getppid();
    sys_write(STDOUT_FILENO, msg5, 10);
    i = 0;
    n = ppid;
    if (n == 0) {
        pid_buf[i++] = '0';
    } else {
        while (n > 0) {
            pid_buf[i++] = '0' + (n % 10);
            n /= 10;
        }
        // Reverse
        for (int j = 0; j < i / 2; j++) {
            char tmp = pid_buf[j];
            pid_buf[j] = pid_buf[i - 1 - j];
            pid_buf[i - 1 - j] = tmp;
        }
    }
    sys_write(STDOUT_FILENO, pid_buf, i);
    sys_write(STDOUT_FILENO, newline, 1);

    // Try to open /test.txt
    sys_write(STDOUT_FILENO, msg2, 21);
    int64_t fd = sys_open("/test.txt", O_RDONLY);

    if (fd >= 0) {
        // Successfully opened
        sys_write(STDOUT_FILENO, msg3, 16);

        // Read and print file contents
        char buf[256];
        int64_t bytes_read = sys_read(fd, buf, 255);
        if (bytes_read > 0) {
            sys_write(STDOUT_FILENO, buf, bytes_read);
        }
        sys_write(STDOUT_FILENO, newline, 1);

        // Close the file
        sys_close(fd);
    } else {
        // Failed to open
        const char *err = "Failed to open /test.txt\n";
        sys_write(STDOUT_FILENO, err, 25);
    }

    // Yield a few times
    for (int i = 0; i < 5; i++) {
        sys_yield();
    }

    // Print completion message
    sys_write(STDOUT_FILENO, done, 26);

    // Exit cleanly
    sys_exit(0);
}
