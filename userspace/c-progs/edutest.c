// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// edutest: ask the edu driver (/svc/edu) to run its checks (registers
// through a VMO, an MSI, DMA through a BTI) and print its report. Exit
// code: the number of failed checks; 2 without an edu device.
//
// QEMU: -device edu,dma_mask=0xffffffff

#include <errno.h>
#include <rustux/service.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    int quiet = argc > 1 && !strcmp(argv[1], "-q");
    if (argc > 1 && !quiet) {
        printf("usage: edutest [-q]\nRun the edu test driver's checks (QEMU -device edu,dma_mask=0xffffffff).\n"
               "-q: print only failures\n");
        return strcmp(argv[1], "--help") ? 2 : 0;
    }
    rx_handle_t ch;
    int r = rx_service_connect("/svc/edu", &ch);
    if (r < 0) {
        fprintf(stderr, "edutest: no edu driver (%s); run QEMU with -device edu,dma_mask=0xffffffff\n", strerror(-r));
        return 2;
    }
    if (rx_channel_write(ch, 0, "run", 3, 0, 0) != RX_OK) {
        fprintf(stderr, "edutest: the driver went away\n");
        return 2;
    }
    static char buf[8192];
    uint32_t nb = 0, nh = 0, pending;
    for (;;) {
        rx_status_t s = rx_channel_read(ch, 0, buf, 0, sizeof buf - 1, 0, &nb, &nh);
        if (s == RX_OK) break;
        if (s != RX_ERR_SHOULD_WAIT) {
            fprintf(stderr, "edutest: the driver went away\n");
            return 2;
        }
        rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pending);
    }
    if (nb < 4) return 2;
    int32_t failed;
    memcpy(&failed, buf, 4);
    buf[nb] = 0;
    if (!quiet || failed) fputs(buf + 4, stdout);
    return failed;
}
