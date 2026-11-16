// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The socket protocol between the C library and the network service
// (network step D2, docs/NETWORK.md). Each socket is a channel to netd,
// a connection to /svc/netsock: its control channel. A TCP connection's
// bytes travel on a kernel socket pair besides (the "data socket"): the
// kernel buffers them (64 KiB each way), which is the flow control, and
// its signals are what poll waits on. Half-closing is
// RX_SOCKET_SHUTDOWN_WRITE on the data socket: from the program, a FIN;
// from netd, the peer's FIN reaching the program as end-of-stream. UDP
// datagrams are messages on the control channel.
//
//   program -> netd        reply
//   OPEN a=type            OPEN status          first, on a new connection
//   BIND addr              BIND status, addr = the address bound
//   LISTEN a=backlog       LISTEN status
//   CONNECT addr (+ the    CONNECT status, addr = local, addr2 = peer
//     data socket's netd     (TCP: once connected or failed)
//     end, for TCP)
//   SENDTO addr + data     -                    a datagram (addr port 0: the connected peer)
//   SETOPT a=opt b=value   -
//
//   netd -> program
//   ACCEPTED addr = peer, addr2 = local, handles: the data socket and the
//            control channel of a new connection (on a listening socket)
//   DGRAM    addr = from + data
//   ERROR    status: the connection failed (ECONNRESET, ETIMEDOUT, ...)
//
// Statuses are 0 or a negative errno (Linux's numbers).

#ifndef RUSTUX_NETSOCK_H
#define RUSTUX_NETSOCK_H

#include <stdint.h>

typedef struct {
    uint16_t family;    // AF_INET (2); AF_INET6 (10) later
    uint16_t port;      // host byte order
    uint8_t addr[16];   // IPv4 in the first 4 bytes
} rx_netaddr_t;

typedef struct {
    uint32_t op;
    int32_t status;
    uint32_t a, b;
    rx_netaddr_t addr, addr2;
} rx_netsock_msg_t;     // 56 bytes, then the data

#define RX_NETSOCK_OPEN     1u
#define RX_NETSOCK_BIND     2u
#define RX_NETSOCK_LISTEN   3u
#define RX_NETSOCK_CONNECT  4u
#define RX_NETSOCK_ACCEPTED 5u
#define RX_NETSOCK_SENDTO   6u
#define RX_NETSOCK_DGRAM    7u
#define RX_NETSOCK_ERROR    8u
#define RX_NETSOCK_SETOPT   9u

#define RX_NETSOCK_STREAM 1u
#define RX_NETSOCK_DGRAM_TYPE 2u

// SETOPT options
#define RX_NETSOCK_OPT_NODELAY   1u
#define RX_NETSOCK_OPT_KEEPALIVE 2u
#define RX_NETSOCK_OPT_REUSEADDR 3u

// The largest datagram (UDP payload) netd carries
#define RX_NETSOCK_MAX_DGRAM 65000u

// One request on netd's control protocol (/svc/net: "links", "addrs",
// "routes", "dns", ...; docs/NETWORK.md), for programs that are not C
// tools (the panel and Settings, step D6): the answer into out,
// NUL-terminated, its last line "ok" or "error: ...". The connection is
// kept for the next request. Returns the answer's length, or -1 (errno
// ENETDOWN) if netd cannot be reached.
#include <stddef.h>
int rx_net_control(const char *req, char *out, size_t cap);

#endif
