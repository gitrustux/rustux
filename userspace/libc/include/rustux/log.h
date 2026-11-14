// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The system log (step 18): logd serves /svc/log and writes what it is
// sent to /var/log/system.log (docs/SERVICES.md). syslog() and logger use
// it; init gives each service's stdout and stderr to it.
//
//   RX_LOG_OP_WRITE   rx_log_msg_t + the text (no reply). The pid and uid
//                     recorded are the sender's, from the kernel.
//   RX_LOG_OP_STREAM  rx_log_msg_t (pid, uid: whose output it is) + one
//                     socket handle: every line read from the socket is a
//                     message (root only: init, for services)
//
// The file: one message per line, fields separated by tabs:
//   TIME BOOT PRIORITY TAG PID UID MESSAGE
// TIME is UTC (2026-11-13T19:42:01.123Z), BOOT counts boots (1, 2, ...),
// PRIORITY is syslog's (0 emerg ... 7 debug), TAG the service or program
// ("kernel" for the kernel's own messages). Tabs and control characters in
// MESSAGE become spaces. Rotated at 1 MiB: system.log.1 (newest) to .4.

#ifndef RUSTUX_LOG_H
#define RUSTUX_LOG_H

#include <stdint.h>

#define RX_LOG_OP_WRITE  1
#define RX_LOG_OP_STREAM 2

#define RX_LOG_TAG_MAX 32
#define RX_LOG_TEXT_MAX 2000

typedef struct {
    uint32_t op;
    uint32_t priority;  // 0-7
    uint64_t pid;       // STREAM only
    uint32_t uid;       // STREAM only
    uint32_t _pad;
    char tag[RX_LOG_TAG_MAX];
} rx_log_msg_t;

#define RX_LOG_FILE "/var/log/system.log"
#define RX_LOG_BOOT "/var/log/boot.count"
#define RX_LOG_KEEP 4

#endif
