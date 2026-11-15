// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Network card drivers and the network service (NETWORK.md step D1,
// docs/NETWORK.md). A driver (virtio-net, ...) moves Ethernet frames and
// nothing more: it connects to /svc/netdev, which netd publishes (again
// and again until netd is there, and again if netd restarts), says what
// it is with HELLO, then each message is one frame or one event:
//
//   driver -> netd   HELLO   a = MTU, then rx_netdev_hello_t
//                    FRAME   a frame received (without the FCS)
//                    LINK    a = 1 up, 0 down
//   netd -> driver   FRAME   a frame to send
//
// A message is rx_netdev_msg_t followed by its data. Frames are at most
// RX_NETDEV_MAX_FRAME bytes; a driver drops what it cannot queue.

#ifndef RUSTUX_NETDEV_H
#define RUSTUX_NETDEV_H

#include <stdint.h>

typedef struct {
    uint32_t op;
    uint32_t a;
} rx_netdev_msg_t;

typedef struct {
    uint8_t mac[6];
    uint16_t link;          // 1 up, 0 down
    char driver[24];        // "virtio-net 00:03.0"
} rx_netdev_hello_t;

#define RX_NETDEV_HELLO 1u
#define RX_NETDEV_FRAME 2u
#define RX_NETDEV_LINK  3u

#define RX_NETDEV_MAX_FRAME 1518  // 1500 bytes of payload, the header, a VLAN tag

#endif
