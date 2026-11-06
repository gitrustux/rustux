// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// nvme: NVM Express disks (step 13), the laptop's internal disk.
//
// READ-ONLY until the installer (checklist step 20): this driver has no
// write path at all. It never sends a write, write-zeroes, format or
// sanitize command, and it gives the block library no write function, so
// every write request is refused with EROFS. It does reset and enable the
// controller and creates one I/O queue pair, which changes nothing on the
// medium.
//
// One admin queue and one I/O queue pair on MSI-X vector 0 (MSI, or
// polling, without MSI-X); one command at a time; namespace 1 (or the
// first active one) as nvme#n1, served through <rustux/blockdev.h>.
// Spec: "NVM Express Base Specification 2.0", sections 3.1, 3.5, 5.

#include <errno.h>
#include <rustux/blockdev.h>
#include <rustux/driver.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

// Controller registers
#define R_CAP   0x00
#define R_VS    0x08
#define R_INTMS 0x0C
#define R_CC    0x14
#define R_CSTS  0x1C
#define R_AQA   0x24
#define R_ASQ   0x28
#define R_ACQ   0x30

#define CC_EN      1u
#define CSTS_RDY   1u
#define CSTS_CFS   2u

// Opcodes
#define A_CREATE_SQ 0x01
#define A_CREATE_CQ 0x05
#define A_IDENTIFY  0x06
#define IO_READ     0x02

#define QD        16              // entries per queue
#define DATA_SIZE (128u * 1024u)  // largest transfer we set up

struct cqe { uint32_t dw0, dw1; uint16_t sqhd, sqid; uint16_t cid, status; };

struct queue {
    rx_dma_buf_t sq, cq;
    uint16_t id, tail, head, phase, cid;
    volatile uint32_t *sq_db, *cq_db;
};

static const char *where = "?";
static rx_handle_t dev, irq;
static volatile uint8_t *regs;
static uint32_t stride;
static int polling;
static struct queue admin, io;
static rx_dma_buf_t data, prp;      // data buffer; a PRP list for transfers over 8 KiB
static uint32_t nsid = 1, lba_size;
static uint64_t ns_blocks;
static uint32_t max_bytes = DATA_SIZE;

static uint32_t rd32(uint32_t o) { return *(volatile uint32_t *)(regs + o); }
static uint64_t rd64(uint32_t o) { return rd32(o) | (uint64_t)rd32(o + 4) << 32; }
static void wr32(uint32_t o, uint32_t v) { *(volatile uint32_t *)(regs + o) = v; }
static void wr64(uint32_t o, uint64_t v) { wr32(o, (uint32_t)v); wr32(o + 4, (uint32_t)(v >> 32)); }

static void sleep_ms(long ms) {
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000 };
    nanosleep(&ts, 0);
}

static int queue_init(struct queue *q, rx_handle_t bti, uint16_t id) {
    q->id = id;
    q->phase = 1;
    if (rx_dma_alloc(bti, QD * 64, 0, &q->sq) < 0 || rx_dma_alloc(bti, QD * 16, 0, &q->cq) < 0) return -ENOMEM;
    q->sq_db = (volatile uint32_t *)(regs + 0x1000 + (2u * id) * stride);
    q->cq_db = (volatile uint32_t *)(regs + 0x1000 + (2u * id + 1) * stride);
    return 0;
}

// Submit `cmd` (64 bytes; the command id is filled in) and wait for its
// completion: its status (0 = success) or -ETIMEDOUT; *dw0 gets DW0
static int submit(struct queue *q, uint32_t *cmd, uint32_t *dw0, uint32_t timeout_ms) {
    uint16_t cid = q->cid++;
    cmd[0] = (cmd[0] & 0xFFFF) | (uint32_t)cid << 16;
    memcpy((void *)(q->sq.va + (size_t)q->tail * 64), cmd, 64);
    q->tail = (uint16_t)((q->tail + 1) % QD);
    rx_dma_barrier();
    *q->sq_db = q->tail;
    volatile struct cqe *c = (volatile struct cqe *)(q->cq.va + (size_t)q->head * 16);
    uint32_t waited = 0;
    while ((c->status & 1) != q->phase) {
        if (polling) {
            sleep_ms(1);
            waited++;
        } else {
            int r = rx_drv_wait_irq(irq, 100);
            waited += 100;
            if (r < 0) polling = 1;
        }
        if (waited >= timeout_ms) {
            fprintf(stderr, "nvme %s: command %#x timed out\n", where, cmd[0] & 0xFF);
            return -ETIMEDOUT;
        }
    }
    rx_dma_barrier();
    uint16_t st = c->status >> 1;
    if (dw0) *dw0 = c->dw0;
    q->head = (uint16_t)((q->head + 1) % QD);
    if (q->head == 0) q->phase ^= 1;
    *q->cq_db = q->head;
    if (c->cid != cid) fprintf(stderr, "nvme %s: completion for command %u, expected %u\n", where, c->cid, cid);
    return st;
}

// The data buffer as PRP entries for `bytes`
static void set_prps(uint32_t *cmd, uint32_t bytes) {
    uint64_t p1 = data.pa, p2 = 0;
    if (bytes > 4096) {
        if (bytes <= 8192) {
            p2 = data.pa + 4096;
        } else {
            volatile uint64_t *list = (volatile uint64_t *)prp.va;
            for (uint32_t k = 1; k * 4096u < bytes; k++) list[k - 1] = data.pa + (uint64_t)k * 4096;
            p2 = prp.pa;
        }
    }
    cmd[6] = (uint32_t)p1;
    cmd[7] = (uint32_t)(p1 >> 32);
    cmd[8] = (uint32_t)p2;
    cmd[9] = (uint32_t)(p2 >> 32);
}

static int identify(uint32_t cns, uint32_t id) {
    uint32_t cmd[16] = { 0 };
    cmd[0] = A_IDENTIFY;
    cmd[1] = id;
    set_prps(cmd, 4096);
    cmd[10] = cns;
    memset((void *)data.va, 0, 4096);
    return submit(&admin, cmd, 0, 5000);
}

static int rd(void *ctx, uint64_t block, uint32_t count, void *buf) {
    (void)ctx;
    uint32_t cmd[16] = { 0 };
    cmd[0] = IO_READ;
    cmd[1] = nsid;
    set_prps(cmd, count * lba_size);
    cmd[10] = (uint32_t)block;
    cmd[11] = (uint32_t)(block >> 32);
    cmd[12] = count - 1;
    int st = submit(&io, cmd, 0, 30000);
    if (st != 0) {
        if (st > 0) fprintf(stderr, "nvme %s: read of %u blocks at %llu failed (status %#x)\n", where, count, (unsigned long long)block, st);
        return st < 0 ? st : -EIO;
    }
    memcpy(buf, (const void *)data.va, (size_t)count * lba_size);
    return 0;
}

// Copy an ASCII field, trimming the space padding
static void field(char *out, size_t cap, const volatile uint8_t *in, size_t len) {
    size_t n = len;
    while (n && (in[n - 1] == ' ' || in[n - 1] == 0)) n--;
    size_t s = 0;
    while (s < n && in[s] == ' ') s++;
    if (n - s >= cap) n = s + cap - 1;
    for (size_t i = s; i < n; i++) out[i - s] = (char)in[i];
    out[n - s] = 0;
}

static int fail(const char *what) {
    fprintf(stderr, "nvme %s: %s\n", where, what);
    return 1;
}

int main(int argc, char **argv) {
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) return fail("started without a device (devmgr starts this driver)");
    rx_pci_bar_t bar;
    regs = rx_drv_map_bar(dev, 0, &bar);
    if (!regs) return fail("cannot map BAR 0");
    rx_handle_t bti;
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    uint64_t cap = rd64(R_CAP);
    stride = 4u << ((cap >> 32) & 0xF);
    uint32_t timeout = (uint32_t)((cap >> 24) & 0xFF) * 500 + 500;  // CAP.TO is in 500 ms units
    if (!((cap >> 37) & 1)) return fail("the controller has no NVM command set");
    if (((cap >> 48) & 0xF) > 0) return fail("the controller's smallest page is above 4 KiB");
    uint32_t mqes = (uint32_t)(cap & 0xFFFF) + 1;
    if (mqes < QD) return fail("queues are too short");

    // Disable, set up the admin queue, enable
    if (rd32(R_CC) & CC_EN) {
        wr32(R_CC, rd32(R_CC) & ~CC_EN);
        for (uint32_t t = 0; (rd32(R_CSTS) & CSTS_RDY) && t < timeout; t += 10) sleep_ms(10);
        if (rd32(R_CSTS) & CSTS_RDY) return fail("the controller does not stop");
    }
    if (queue_init(&admin, bti, 0) < 0 || queue_init(&io, bti, 1) < 0 || rx_dma_alloc(bti, DATA_SIZE, 0, &data) < 0 ||
        rx_dma_alloc(bti, 4096, 0, &prp) < 0)
        return fail("no DMA memory");
    rx_drv_bus_master(dev, 1);
    uint32_t mode = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_ANY, 0, &irq, &mode) < 0) polling = 1;
    wr32(R_AQA, (QD - 1) << 16 | (QD - 1));
    wr64(R_ASQ, admin.sq.pa);
    wr64(R_ACQ, admin.cq.pa);
    // NVM command set, 4 KiB pages, round robin, 64-byte SQ and 16-byte CQ entries
    wr32(R_CC, CC_EN | 6u << 16 | 4u << 20);
    for (uint32_t t = 0; !(rd32(R_CSTS) & (CSTS_RDY | CSTS_CFS)) && t < timeout; t += 10) sleep_ms(10);
    if (rd32(R_CSTS) & CSTS_CFS) return fail("controller fatal status after enabling");
    if (!(rd32(R_CSTS) & CSTS_RDY)) return fail("the controller does not become ready");

    // Identify the controller: model, serial, transfer limit, namespaces
    if (identify(1, 0) != 0) return fail("identify controller failed");
    char model[48], serial[24];
    field(model, sizeof model, data.va + 24, 40);
    field(serial, sizeof serial, data.va + 4, 20);
    uint8_t mdts = data.va[77];
    uint32_t nn = *(volatile uint32_t *)(data.va + 516);
    if (mdts && mdts < 20 && (4096u << mdts) < max_bytes) max_bytes = 4096u << mdts;

    // The first active namespace (normally 1)
    for (nsid = 1; nsid <= (nn ? nn : 1) && nsid <= 16; nsid++) {
        if (identify(0, nsid) == 0 && *(volatile uint64_t *)data.va) break;
    }
    if (nsid > (nn ? nn : 1) || nsid > 16) return fail("no active namespace");
    ns_blocks = *(volatile uint64_t *)data.va;
    uint8_t flbas = data.va[26] & 0xF;
    uint32_t lbaf = *(volatile uint32_t *)(data.va + 128 + 4 * flbas);
    uint32_t lbads = (lbaf >> 16) & 0xFF, ms = lbaf & 0xFFFF;
    if (lbads < 9 || lbads > 12) return fail("unsupported block size");
    if (ms && !(data.va[26] & 0x10)) return fail("blocks with separate metadata are not supported");
    lba_size = 1u << lbads;

    // The I/O queue pair, both on interrupt vector 0
    uint32_t cmd[16] = { 0 };
    cmd[0] = A_CREATE_CQ;
    cmd[6] = (uint32_t)io.cq.pa;
    cmd[7] = (uint32_t)(io.cq.pa >> 32);
    cmd[10] = (QD - 1) << 16 | 1;
    cmd[11] = polling ? 1u : (0u << 16 | 2 | 1);  // vector 0, interrupts enabled, contiguous
    if (submit(&admin, cmd, 0, 5000) != 0) return fail("cannot create the I/O completion queue");
    memset(cmd, 0, sizeof cmd);
    cmd[0] = A_CREATE_SQ;
    cmd[6] = (uint32_t)io.sq.pa;
    cmd[7] = (uint32_t)(io.sq.pa >> 32);
    cmd[10] = (QD - 1) << 16 | 1;
    cmd[11] = 1u << 16 | 1;  // completion queue 1, contiguous
    if (submit(&admin, cmd, 0, 5000) != 0) return fail("cannot create the I/O submission queue");

    rx_block_info_t info = { 0 };
    snprintf(info.name, sizeof info.name, "nvme#n%u", nsid);
    info.block_count = ns_blocks;
    info.block_size = lba_size;
    info.flags = RX_BLOCK_FLAG_READONLY;
    strcpy(info.driver, "nvme");
    snprintf(info.model, sizeof info.model, "%s", model);
    (void)serial;
    rx_blockdev_ops_t ops = { rd, 0, 0 };   // no write, no flush: read-only
    int r = rx_blockdev_publish(&ops, 0, &info, max_bytes / lba_size, 0);
    if (r < 0) {
        fprintf(stderr, "nvme %s: cannot publish the disk: %s\n", where, strerror(-r));
        return 1;
    }
    return rx_blockdev_run();
}
