// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// drvtest: an ordinary program cannot touch hardware. The driver calls
// need the root resource (which only init has), and I/O ports are closed
// unless a process was granted them. The last check reads the keyboard port
// directly: the CPU must refuse, so this program ends killed by the
// exception (the shell shows "[exit code -1025]").

#include "rx.h"

static int passed, failed;

static void check(const char *name, int ok, int64_t got) {
    rx_puts(ok ? "  ok   " : "  FAIL ");
    rx_puts(name);
    if (!ok) {
        rx_puts(" (got ");
        rx_putd(got);
        rx_puts(")");
    }
    rx_puts("\n");
    if (ok) passed++; else failed++;
}

int main(void) {
    rx_handle_t ev, irq;
    int64_t r;
    rx_puts("drvtest: hardware access without the root resource\n");
    check("we were not given a resource", rx_take_startup_handle(RX_PA_ROOT_RESOURCE) == 0, 0);
    rx_event_create(0, &ev);
    r = rx_interrupt_create(0, 1, 0, &irq);
    check("interrupt_create without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_interrupt_create(ev, 1, 0, &irq);
    check("interrupt_create with an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_ioports_request(ev, 0x60, 1);
    check("ioports_request with an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_interrupt_wait(ev, 0);
    check("interrupt_wait on an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    rx_puts("drvtest: ");
    rx_putd(passed);
    rx_puts(" passed, ");
    rx_putd(failed);
    rx_puts(failed ? " FAILED\n" : " failed\n");
    rx_puts("drvtest: now reading port 0x60 directly; the CPU should stop us...\n");
    volatile uint8_t v = rx_inb(0x60);
    (void)v;
    rx_puts("  FAIL port 0x60 was readable without permission\n");
    rx_exit(1);
}
