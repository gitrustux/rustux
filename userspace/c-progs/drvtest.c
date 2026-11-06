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
    // Step 12: device memory, MSI, DMA and ACPI need the root resource too
    rx_handle_t h;
    r = rx_interrupt_create(0, 0, RX_INTERRUPT_MSI, &h);
    check("interrupt_create(MSI) without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_vmo_create_physical(0, 0xFEC00000, 4096, &h);
    check("vmo_create_physical without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_vmo_create_physical(ev, 0xFEC00000, 4096, &h);
    check("vmo_create_physical with an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_bti_create(0, 0, 0, &h);
    check("bti_create without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    r = rx_vmo_create_contiguous(ev, 4096, 0, &h);
    check("vmo_create_contiguous with an event (not a BTI) -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    uint64_t addr;
    r = rx_bti_pin(ev, RX_BTI_PERM_READ, ev, 0, 4096, &addr, &h);
    check("bti_pin with an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_resource_create_range(ev, RX_RSRC_KIND_IOPORT, 0x60, 1, &h);
    check("resource_create(IOPORT) from an event -> WRONG_TYPE", r == RX_ERR_WRONG_TYPE, r);
    r = rx_acpi_table_read(0, "MCFG", 0, 0, 0);
    check("acpi_table_read without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    rx_usb_info_t ui[2];
    r = rx_usb_list(0, ui, 2);
    check("usb_list without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    rx_handle_t uh = 0;
    r = rx_usb_open(0, 1, &uh);
    check("usb_open without a resource -> BAD_HANDLE", r == RX_ERR_BAD_HANDLE, r);
    uint8_t setup[8] = { 0 }, ub[8];
    r = rx_usb_bulk(ev, RX_USB_IN, ub, 8, 100);
    check("usb_bulk with an event (not a USB device) -> refused", r == RX_ERR_WRONG_TYPE || r == RX_ERR_ACCESS_DENIED, r);
    r = rx_usb_control(ev, setup, 0, 0);
    check("usb_control with an event -> refused", r == RX_ERR_WRONG_TYPE || r == RX_ERR_ACCESS_DENIED, r);
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
