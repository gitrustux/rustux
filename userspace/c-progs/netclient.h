// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// A request to netd's control protocol (/svc/net, docs/NETWORK.md): one
// line of text out, the answer back (its last line "ok" or "error: ...";
// a ping is answered "reply ..." or "timeout"). Shared by ip and ping.

#ifndef NETCLIENT_H
#define NETCLIENT_H

#include <rustux/service.h>
#include <rustux/syscalls.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static rx_handle_t net_ch;  // 0 until connected (0 is never a handle)

static void net_connect(const char *prog) {
    int r = rx_service_connect("/svc/net", &net_ch);
    if (r < 0) {
        fprintf(stderr, "%s: cannot reach the network service (/svc/net): %s\n", prog, strerror(-r));
        fprintf(stderr, "%s: is netd running? (svc status netd)\n", prog);
        exit(1);
    }
}

static int net_send(const char *req) {
    return rx_channel_write(net_ch, 0, req, (uint32_t)strlen(req), 0, 0) == RX_OK ? 0 : -1;
}

// The next answer into buf (NUL-terminated): its length, or -1 if netd
// has gone
static int net_recv(char *buf, uint32_t size) {
    for (;;) {
        uint32_t nb = 0;
        rx_status_t s = rx_channel_read(net_ch, 0, buf, 0, size - 1, 0, &nb, 0);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t seen = 0;
            rx_object_wait_one(net_ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &seen);
            if (!(seen & RX_CHANNEL_READABLE) && (seen & RX_PEER_CLOSED)) return -1;
            continue;
        }
        if (s != RX_OK) return -1;
        buf[nb] = 0;
        return (int)nb;
    }
}

// Request and answer; an "error: ..." answer is printed and ends the program
__attribute__((unused)) static char *net_request(const char *prog, const char *req) {
    static char buf[65536];
    if (!net_ch) net_connect(prog);
    if (net_send(req) < 0 || net_recv(buf, sizeof buf) < 0) {
        fprintf(stderr, "%s: the network service did not answer\n", prog);
        exit(1);
    }
    if (!strncmp(buf, "error: ", 7)) {
        char *nl = strchr(buf, '\n');
        if (nl) *nl = 0;
        fprintf(stderr, "%s: %s\n", prog, buf + 7);
        exit(2);
    }
    return buf;
}

#endif
