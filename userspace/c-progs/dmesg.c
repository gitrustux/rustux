// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// dmesg: print the kernel log (the last 64 KiB of what the kernel printed
// for debugging since boot)

#include <rustux/process.h>
#include <unistd.h>

int main(void) {
    rx_handle_t log;
    if (rx_debuglog_create(0, RX_LOG_FLAG_READABLE, &log) != RX_OK) return 1;
    char buf[4096];
    for (;;) {
        rx_status_t n = rx_debuglog_read(log, 0, buf, sizeof buf);
        if (n <= 0) break;
        if (write(1, buf, (size_t)n) != n) return 1;
    }
    return 0;
}
