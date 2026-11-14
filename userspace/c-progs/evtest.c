// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// evtest: print input events (DESKTOP.md step F3). Takes the focus from
// the input service, so the keys typed go to evtest alone, and prints
// every key and pointer event until Esc is pressed (or SECONDS pass).
// Root only.
//
//   evtest [-t SECONDS]

#include <errno.h>
#include <rustux/input.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void show_text(const char *t) {
    for (; *t; t++) {
        unsigned char ch = (unsigned char)*t;
        if (ch == 0x1B) printf("\\e");
        else if (ch == '\r') printf("\\r");
        else if (ch < 0x20 || ch == 0x7F) printf("^%c", ch ^ 0x40);
        else putchar(ch);
    }
}

int main(int argc, char **argv) {
    long seconds = 0;
    if (argc == 3 && !strcmp(argv[1], "-t")) seconds = atol(argv[2]);
    else if (argc != 1) {
        fprintf(stderr, "usage: evtest [-t SECONDS]\n");
        return 2;
    }
    rx_handle_t ch;
    if (rx_service_connect("/svc/input", &ch) < 0) {
        fprintf(stderr, "evtest: no input service\n");
        return 1;
    }
    rx_input_req_t req = { RX_INPUT_OP_GRAB, 0 };
    int32_t st = -EIO;
    uint32_t nb = 0;
    rx_channel_write(ch, 0, &req, sizeof req, 0, 0);
    for (;;) {
        rx_status_t s = rx_channel_read(ch, 0, &st, 0, sizeof st, 0, &nb, 0);
        if (s == RX_OK) break;
        uint32_t pend = 0;
        if (s != RX_ERR_SHOULD_WAIT ||
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend) != RX_OK) {
            st = -EPIPE;
            break;
        }
    }
    if (st < 0) {
        fprintf(stderr, "evtest: %s\n", strerror(-st));
        return 1;
    }
    printf("evtest: reading input%s; Esc ends\n", seconds ? " (for a while)" : "");
    fflush(stdout);
    uint64_t end = seconds ? rx_clock_get_monotonic() + (uint64_t)seconds * 1000000000ull : RX_TIME_INFINITE;
    for (;;) {
        uint32_t pend = 0;
        rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, end, &pend);
        if (!(pend & RX_CHANNEL_READABLE)) break;
        rx_input_event_t e;
        if (rx_channel_read(ch, 0, &e, 0, sizeof e, 0, &nb, 0) != RX_OK || nb != sizeof e) continue;
        if (e.type == RX_INPUT_KEY) {
            printf("key 0x%02x %s mods 0x%02x", e.code,
                   e.flags & RX_INPUT_REPEAT ? "repeat" : e.flags & RX_INPUT_PRESSED ? "down" : "up", e.mods);
            if (e.text[0]) {
                printf(" text \"");
                show_text(e.text);
                printf("\"");
            }
            printf("\n");
            if (e.code == 0x29 && (e.flags & RX_INPUT_PRESSED)) break;
        } else if (e.type == RX_INPUT_POINTER) {
            printf("pointer %s %d %d buttons 0x%x wheel %d\n", e.flags & RX_INPUT_ABSOLUTE ? "at" : "moved", e.x, e.y,
                   e.code, e.wheel);
        }
        fflush(stdout);
    }
    rx_handle_close(ch); // the focus goes back
    printf("evtest: done\n");
    return 0;
}
