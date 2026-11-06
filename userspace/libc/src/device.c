// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Client side of the device manager and block protocols
// (<rustux/device.h>, <rustux/block.h>).

#include <errno.h>
#include <rustux/device.h>
#include <rustux/service.h>
#include <stdlib.h>
#include <string.h>

static uint32_t next_txid = 1;

// Send `msg` with handle `h`, wait for the reply into `buf`
static int roundtrip(rx_handle_t ch, const void *msg, size_t len, rx_handle_t h, void *buf, size_t cap,
                     uint32_t *got, rx_handle_t *hout) {
    if (rx_channel_write(ch, 0, msg, (uint32_t)len, h ? &h : 0, h ? 1 : 0) != RX_OK) return -EPIPE;
    for (;;) {
        rx_handle_t hh = 0;
        uint32_t nh = 0;
        rx_status_t s = rx_channel_read(ch, 0, buf, &hh, (uint32_t)cap, 1, got, &nh);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t pending = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pending);
            continue;
        }
        if (s != RX_OK) return -EPIPE;
        if (hout) *hout = nh ? hh : 0;
        else if (nh) rx_handle_close(hh);
        return 0;
    }
}

int rx_dev_call(rx_handle_t ch, uint32_t op, const void *req, size_t len, rx_handle_t h,
                void *out, size_t cap, size_t *outlen, rx_handle_t *hout) {
    if (outlen) *outlen = 0;
    if (hout) *hout = 0;
    size_t mlen = sizeof(rx_dev_hdr_t) + len;
    if (mlen > 65536) return -E2BIG;
    char *msg = malloc(mlen), *rep = malloc(65536);
    if (!msg || !rep) {
        free(msg);
        free(rep);
        if (h) rx_handle_close(h);
        return -ENOMEM;
    }
    rx_dev_hdr_t hdr = { next_txid++, op };
    memcpy(msg, &hdr, sizeof hdr);
    if (len) memcpy(msg + sizeof hdr, req, len);
    uint32_t got = 0;
    int r = roundtrip(ch, msg, mlen, h, rep, 65536, &got, hout);
    if (r == 0) {
        rx_dev_reply_t rp;
        if (got < sizeof rp) {
            r = -EIO;
        } else {
            memcpy(&rp, rep, sizeof rp);
            r = rp.status;
            size_t n = got - sizeof rp;
            if (n > cap) n = cap;
            if (r == 0 && n && out) memcpy(out, rep + sizeof rp, n);
            if (outlen) *outlen = r == 0 ? n : 0;
        }
    }
    free(msg);
    free(rep);
    return r;
}

int rx_devmgr_connect(rx_handle_t *out) { return rx_service_connect("/svc/devmgr", out); }

int rx_block_open(const char *name, rx_handle_t *out) {
    rx_handle_t dm, c, s;
    int r = rx_devmgr_connect(&dm);
    if (r < 0) return r;
    if (rx_channel_create(0, &c, &s) != RX_OK) {
        rx_handle_close(dm);
        return -ENOMEM;
    }
    r = rx_dev_call(dm, RX_DM_OP_CONNECT_BLOCK, name, strlen(name), s, 0, 0, 0, 0);
    rx_handle_close(dm);
    if (r < 0) {
        rx_handle_close(c);
        return r;
    }
    *out = c;
    return 0;
}

int rx_block_call(rx_handle_t ch, rx_block_req_t *req, const void *data, size_t len, rx_handle_t h,
                  void *out, size_t cap, size_t *outlen) {
    if (outlen) *outlen = 0;
    size_t mlen = sizeof *req + len;
    if (len > RX_BLOCK_MAX_IO) return -E2BIG;
    static char msg[sizeof(rx_block_req_t) + RX_BLOCK_MAX_IO], rep[sizeof(rx_block_reply_t) + RX_BLOCK_MAX_IO + 256];
    req->txid = next_txid++;
    memcpy(msg, req, sizeof *req);
    if (len) memcpy(msg + sizeof *req, data, len);
    uint32_t got = 0;
    int r = roundtrip(ch, msg, mlen, h, rep, sizeof rep, &got, 0);
    if (r < 0) return r;
    rx_block_reply_t rp;
    if (got < sizeof rp) return -EIO;
    memcpy(&rp, rep, sizeof rp);
    if (rp.status < 0) return rp.status;
    size_t n = got - sizeof rp;
    if (n > cap) n = cap;
    if (n && out) memcpy(out, rep + sizeof rp, n);
    if (outlen) *outlen = n;
    return 0;
}
