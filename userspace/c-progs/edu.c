// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// edu: driver for QEMU's "edu" teaching device (-device edu), the test
// driver of step 12. devmgr starts it (/etc/drivers) with the device
// channel; it never sees the root resource. It checks, in userspace:
//
//   - its BAR 0 mapped through a VMO (identification, liveness register,
//     the factorial unit),
//   - an MSI interrupt raised by the device and received with
//     interrupt_wait,
//   - DMA both ways: a contiguous buffer and a pinned ordinary VMO, at
//     the addresses bti_pin returns.
//
// Then it publishes /svc/edu: each request runs the checks again and
// replies with a report (edutest prints it).
//
// The device's default DMA mask is 28 bits; run QEMU with
// -device edu,dma_mask=0xffffffff so buffers anywhere below 4 GiB work.

#include <pthread.h>
#include <rustux/device.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static rx_handle_t dev, irq, bti;
static volatile uint8_t *regs;
static char report[4096];
static size_t rlen;
static int passed, failed, have_root;

static uint32_t rd(uint32_t off) { return *(volatile uint32_t *)(regs + off); }
static void wr(uint32_t off, uint32_t v) { *(volatile uint32_t *)(regs + off) = v; }
static uint64_t rd64(uint32_t off) { return *(volatile uint64_t *)(regs + off); }
static void wr64(uint32_t off, uint64_t v) { *(volatile uint64_t *)(regs + off) = v; }

static void say(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(report + rlen, sizeof report - rlen, fmt, ap);
    va_end(ap);
    if (n > 0) rlen += (size_t)n < sizeof report - rlen ? (size_t)n : sizeof report - rlen - 1;
}

static void check(int ok, const char *what) {
    say("  %s %s\n", ok ? "ok  " : "FAIL", what);
    if (ok) passed++;
    else failed++;
}

// ---- interrupts with a time limit ----

static volatile int waiting, timed_out;

static void *watchdog(void *arg) {
    (void)arg;
    for (;;) {
        struct timespec ts = { 0, 50000000 };
        nanosleep(&ts, 0);
        static int ticks;
        if (!waiting) { ticks = 0; continue; }
        if (++ticks >= 40) { // 2 s
            timed_out = 1;
            rx_interrupt_destroy(irq); // the wait returns CANCELED
            return 0;
        }
    }
}

// Wait for the MSI; returns 1 if it came
static int wait_irq(void) {
    if (timed_out) return 0;
    waiting = 1;
    rx_status_t s = rx_interrupt_wait(irq, 0);
    waiting = 0;
    if (s != RX_OK) return 0;
    rx_interrupt_ack(irq);
    return 1;
}

// ---- the checks ----

static int dma(uint64_t src, uint64_t dst, uint32_t count, int to_ram) {
    wr64(0x80, src);
    wr64(0x88, dst);
    wr64(0x90, count);
    wr64(0x98, 1 | (to_ram ? 2 : 0) | 4); // start, direction, interrupt when done
    int got = wait_irq();
    uint32_t st = rd(0x24);
    wr(0x64, st); // acknowledge (0x100: DMA done)
    for (int k = 0; k < 100000 && (rd64(0x98) & 1); k++) {}
    return got && (st & 0x100);
}

static void run_checks(void) {
    rlen = 0;
    passed = failed = 0;
    rx_handle_t h;
    check(!have_root && rx_vmo_create_physical(bti, 0xFEC00000, 4096, &h) == RX_ERR_WRONG_TYPE,
          "the driver holds no root resource; its BTI cannot make device memory");
    uint32_t id = rd(0x00);
    char line[96];
    snprintf(line, sizeof line, "identification register reads 0x%08x (edu %u.%u)", id, id >> 24, (id >> 16) & 0xFF);
    check((id & 0xFFFF) == 0x00ED, line);
    wr(0x04, 0x12345678);
    check(rd(0x04) == ~0x12345678u, "liveness register inverts what is written");
    wr(0x08, 10);
    for (int k = 0; k < 1000000 && (rd(0x20) & 1); k++) {}
    check(rd(0x08) == 3628800, "factorial unit: 10! = 3628800");

    // MSI
    if (!timed_out) {
        wr(0x60, 0x5A);
        int got = wait_irq();
        uint32_t st = rd(0x24);
        wr(0x64, st);
        check(got && (st & 0x5A) == 0x5A, "MSI raised by the device arrives through interrupt_wait");
    } else {
        check(0, "MSI raised by the device arrives through interrupt_wait (an earlier wait timed out)");
    }

    // DMA: a contiguous buffer
    rx_handle_t vmo = 0, pmt = 0;
    uint64_t pa = 0, va = 0;
    int ok = rx_vmo_create_contiguous(bti, 8192, RX_VMO_CONTIGUOUS_LOW, &vmo) == RX_OK &&
             rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE | RX_BTI_CONTIGUOUS, vmo, 0, 8192, &pa, &pmt) == RX_OK &&
             rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 8192, &va) == RX_OK;
    check(ok && pa, "contiguous DMA buffer created, pinned and mapped");
    if (ok) {
        uint8_t *b = (uint8_t *)va;
        for (int k = 0; k < 4096; k++) b[k] = (uint8_t)(k * 7 + 3);
        memset(b + 4096, 0, 4096);
        int a = dma(pa, 0x40000, 4096, 0);
        int c = dma(0x40000, pa + 4096, 4096, 1);
        check(a && c && !memcmp(b, b + 4096, 4096), "DMA to the device and back (4 KiB, contiguous buffer)");
        rx_vmar_unmap(rx_vmar_root, va, 8192);
        rx_pmt_unpin(pmt);
    }
    if (vmo) rx_handle_close(vmo);

    // DMA: an ordinary VMO, pinned page by page
    vmo = pmt = 0;
    uint64_t pages[2] = { 0, 0 };
    ok = rx_vmo_create(8192, 0, &vmo) == RX_OK &&
         rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE, vmo, 0, 8192, pages, &pmt) == RX_OK &&
         rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 8192, &va) == RX_OK;
    check(ok && pages[0] && pages[1], "ordinary VMO pinned (two page addresses)");
    if (ok) {
        uint8_t *b = (uint8_t *)va;
        for (int k = 0; k < 4096; k++) b[4096 + k] = (uint8_t)(k ^ 0xA5);
        memset(b, 0, 4096);
        int a = 0, c = 0;
        // edu takes 32-bit... any address under its DMA mask; ours are below 4 GiB on QEMU
        if (pages[1] < (1ull << 32) && pages[0] < (1ull << 32)) {
            a = dma(pages[1], 0x40000, 4096, 0);
            c = dma(0x40000, pages[0], 4096, 1);
        }
        check(a && c && !memcmp(b, b + 4096, 4096), "DMA through pinned pages (second page to the device, back into the first)");
        rx_info_bti_t bi;
        int pinned = rx_object_get_info(bti, RX_INFO_BTI, &bi, sizeof bi, 0, 0) == RX_OK ? (int)bi.pmo_count : -1;
        rx_vmar_unmap(rx_vmar_root, va, 8192);
        rx_pmt_unpin(pmt);
        int after = rx_object_get_info(bti, RX_INFO_BTI, &bi, sizeof bi, 0, 0) == RX_OK ? (int)bi.pmo_count : -1;
        check(pinned == 1 && after == 0, "the BTI counts pinned ranges (1, then 0 after pmt_unpin)");
    }
    if (vmo) rx_handle_close(vmo);
    say("edu: %d passed, %d failed\n", passed, failed);
}

// ---- startup ----

static int dev_call(uint32_t op, const void *req, size_t len, void *out, size_t cap, rx_handle_t *h) {
    return rx_dev_call(dev, op, req, len, 0, out, cap, 0, h);
}

int main(int argc, char **argv) {
    const char *where = argc > 1 ? argv[1] : "?";
    dev = rx_take_startup_handle(RX_PA_USER0);
    have_root = rx_take_startup_handle(RX_PA_ROOT_RESOURCE) != 0;
    if (!dev) {
        fprintf(stderr, "edu: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    rx_pci_bar_t bar;
    rx_handle_t vmo = 0;
    uint32_t idx = 0;
    if (dev_call(RX_DEV_OP_GET_BAR, &idx, 4, &bar, sizeof bar, &vmo) < 0 || !vmo) {
        fprintf(stderr, "edu %s: no BAR 0\n", where);
        return 1;
    }
    uint64_t va = 0;
    uint64_t len = (bar.size + (bar.address & 0xFFF) + 0xFFF) & ~0xFFFull;
    if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, len, &va) != RX_OK) {
        fprintf(stderr, "edu %s: cannot map BAR 0\n", where);
        return 1;
    }
    regs = (volatile uint8_t *)(va + (bar.address & 0xFFF));
    uint32_t mode[2] = { RX_DEV_IRQ_MSI, 0 }, used;
    if (dev_call(RX_DEV_OP_GET_INTERRUPT, mode, 8, &used, 4, &irq) < 0 || !irq) {
        fprintf(stderr, "edu %s: no MSI interrupt\n", where);
        return 1;
    }
    if (dev_call(RX_DEV_OP_GET_BTI, 0, 0, 0, 0, &bti) < 0 || !bti) {
        fprintf(stderr, "edu %s: no BTI\n", where);
        return 1;
    }
    uint32_t on = 1;
    dev_call(RX_DEV_OP_BUS_MASTER, &on, 4, 0, 0, 0);
    pthread_t t;
    pthread_create(&t, 0, watchdog, 0);

    run_checks();
    if (failed) fputs(report, stderr);

    rx_handle_t svc, theirs;
    rx_channel_create(0, &svc, &theirs);
    if (rx_service_publish("/svc/edu", theirs) < 0) fprintf(stderr, "edu %s: cannot publish /svc/edu\n", where);

    // Serve /svc/edu: every message on a client connection runs the checks
    rx_handle_t clients[16];
    int nclients = 0;
    for (;;) {
        rx_wait_item_t items[17];
        items[0] = (rx_wait_item_t){ svc, RX_CHANNEL_READABLE, 0 };
        for (int c = 0; c < nclients; c++) items[1 + c] = (rx_wait_item_t){ clients[c], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        if (rx_object_wait_many(items, 1 + (uint64_t)nclients, RX_TIME_INFINITE) != RX_OK) continue;
        for (int c = nclients - 1; c >= 0; c--) {
            if (!items[1 + c].pending) continue;
            char buf[64];
            uint32_t nb = 0, nh = 0;
            rx_handle_t h;
            rx_status_t s = rx_channel_read(clients[c], 0, buf, &h, sizeof buf, 1, &nb, &nh);
            if (s == RX_ERR_SHOULD_WAIT) continue;
            if (nh) rx_handle_close(h);
            if (s != RX_OK) {
                rx_handle_close(clients[c]);
                clients[c] = clients[--nclients];
                continue;
            }
            run_checks();
            int32_t f = failed;
            char out[sizeof report + 4];
            memcpy(out, &f, 4);
            memcpy(out + 4, report, rlen);
            rx_channel_write(clients[c], 0, out, (uint32_t)(4 + rlen), 0, 0);
        }
        if (items[0].pending) {
            rx_svc_connect_t m;
            rx_handle_t h;
            uint32_t nb, nh;
            while (rx_channel_read(svc, 0, &m, &h, sizeof m, 1, &nb, &nh) == RX_OK) {
                if (nh && nclients < 16) clients[nclients++] = h;
                else if (nh) rx_handle_close(h);
            }
        }
    }
}
