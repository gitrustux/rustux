// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// syslog(): one RX_LOG_OP_WRITE message to logd per call (step 18). The
// connection is made at the first message (or openlog with LOG_NDELAY)
// and made again if logd has restarted.

#include <errno.h>
#include <rustux/log.h>
#include <rustux/service.h>
#include <stdio.h>
#include <string.h>
#include <syslog.h>
#include <unistd.h>

static rx_handle_t log_ch;
static char log_ident[RX_LOG_TAG_MAX];
static int log_opt, log_mask = 0xFF;

static int log_connect(void) {
    if (log_ch) return 0;
    return rx_service_connect("/svc/log", &log_ch) < 0 ? -1 : 0;
}

void openlog(const char *ident, int option, int facility) {
    (void)facility;
    log_ident[0] = 0;
    if (ident) snprintf(log_ident, sizeof log_ident, "%s", ident);
    log_opt = option;
    if (option & LOG_NDELAY) log_connect();
}

void closelog(void) {
    if (log_ch) rx_handle_close(log_ch);
    log_ch = 0;
}

int setlogmask(int mask) {
    int old = log_mask;
    if (mask) log_mask = mask;
    return old;
}

void vsyslog(int priority, const char *fmt, va_list ap) {
    int pri = LOG_PRI(priority);
    if (!(log_mask & LOG_MASK(pri))) return;
    int saved = errno;
    struct {
        rx_log_msg_t h;
        char text[RX_LOG_TEXT_MAX];
    } m;
    memset(&m.h, 0, sizeof m.h);
    m.h.op = RX_LOG_OP_WRITE;
    m.h.priority = (uint32_t)pri;
    snprintf(m.h.tag, sizeof m.h.tag, "%s", log_ident[0] ? log_ident : program_invocation_short_name);
    errno = saved;
    int n = vsnprintf(m.text, sizeof m.text, fmt, ap);
    if (n < 0) return;
    if ((size_t)n >= sizeof m.text) n = sizeof m.text - 1;
    while (n > 0 && m.text[n - 1] == '\n') m.text[--n] = 0;
    if (log_opt & LOG_PERROR) fprintf(stderr, "%s: %s\n", m.h.tag, m.text);
    for (int tries = 0; tries < 2; tries++) {
        if (log_connect() < 0) break;
        if (rx_channel_write(log_ch, 0, &m, (uint32_t)(sizeof m.h + (size_t)n), 0, 0) == RX_OK) break;
        closelog(); // logd went away: once more, with a new connection
    }
    errno = saved;
}

void syslog(int priority, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vsyslog(priority, fmt, ap);
    va_end(ap);
}
