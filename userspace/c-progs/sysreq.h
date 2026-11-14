// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// sysreq.h: a request to init's /svc/system (<rustux/auth.h>), step 17.
// 0, or a negative errno (EPERM: not root).

#ifndef SYSREQ_H
#define SYSREQ_H

#include <errno.h>
#include <rustux/auth.h>
#include <rustux/service.h>
#include <string.h>

static __attribute__((unused)) int sysreq(uint32_t op, const void *data, uint32_t len) {
    rx_handle_t ch;
    int r = rx_service_connect("/svc/system", &ch);
    if (r < 0) return r;
    char msg[64];
    if (len > sizeof msg - 4) len = sizeof msg - 4;
    memcpy(msg, &op, 4);
    if (len) memcpy(msg + 4, data, len);
    if (rx_channel_write(ch, 0, msg, 4 + len, 0, 0) != RX_OK) {
        rx_handle_close(ch);
        return -EIO;
    }
    int32_t st = -EIO;
    uint32_t nb = 0;
    for (;;) {
        rx_status_t s = rx_channel_read(ch, 0, &st, 0, sizeof st, 0, &nb, 0);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t pend = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
            continue;
        }
        if (s != RX_OK || nb < 4) st = -EIO;
        break;
    }
    rx_handle_close(ch);
    return st;
}

#endif
