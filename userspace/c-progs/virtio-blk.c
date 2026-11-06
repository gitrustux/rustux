// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// virtio-blk: the disk of QEMU, KVM and most cloud servers (step 13).
// The modern (virtio 1.0) PCI interface: the configuration structures
// that vendor capabilities point into, one split virtqueue, an MSI-X
// interrupt (polling if the device has none). One request at a time,
// served through <rustux/blockdev.h>, which also handles partitions.
//
// devmgr starts it for 1af4:1001 (transitional) and 1af4:1042 (modern).
// Spec: "Virtual I/O Device (VIRTIO) Version 1.2", sections 2, 4.1, 5.2.

#include <errno.h>
#include <rustux/blockdev.h>
#include <rustux/driver.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

// Capability types (cfg_type)
#define CAP_COMMON 1
#define CAP_NOTIFY 2
#define CAP_ISR    3
#define CAP_DEVICE 4

// Common configuration
#define C_DFSELECT  0x00
#define C_DF        0x04
#define C_GFSELECT  0x08
#define C_GF        0x0C
#define C_MSIX_CFG  0x10
#define C_STATUS    0x14
#define C_GEN       0x15
#define C_QSELECT   0x16
#define C_QSIZE     0x18
#define C_QMSIX     0x1A
#define C_QENABLE   0x1C
#define C_QNOTIFY   0x1E
#define C_QDESC     0x20
#define C_QDRIVER   0x28
#define C_QDEVICE   0x30

#define S_ACK       1
#define S_DRIVER    2
#define S_DRIVER_OK 4
#define S_FEAT_OK   8
#define S_FAILED    128

#define F_RO        (1ull << 5)
#define F_BLK_SIZE  (1ull << 6)
#define F_FLUSH     (1ull << 9)
#define F_VERSION_1 (1ull << 32)

#define NO_VECTOR 0xFFFF

#define T_IN    0
#define T_OUT   1
#define T_FLUSH 4
#define T_GET_ID 8

#define QSIZE 16                 // descriptors we use (3 per request)
#define DATA_SIZE (128u * 1024u) // largest transfer

#define VRING_DESC_F_NEXT  1
#define VRING_DESC_F_WRITE 2

struct desc { uint64_t addr; uint32_t len; uint16_t flags, next; };

static const char *where = "?";
static rx_handle_t dev, irq;
static volatile uint8_t *common, *notify_at, *devcfg;
static int polling;
static uint16_t qsize, last_used;
static rx_dma_buf_t ring, data;
static volatile struct desc *descs;
static volatile uint16_t *avail;      // flags, idx, ring[qsize], used_event
static volatile uint16_t *used_hdr;   // flags, idx; then {u32 id, u32 len}[qsize]
static volatile uint8_t *hdr, *status;
static uint64_t hdr_pa, status_pa;

static uint8_t r8(volatile uint8_t *b, uint32_t o) { return b[o]; }
static uint16_t r16(volatile uint8_t *b, uint32_t o) { return *(volatile uint16_t *)(b + o); }
static uint32_t r32(volatile uint8_t *b, uint32_t o) { return *(volatile uint32_t *)(b + o); }
static void w8(volatile uint8_t *b, uint32_t o, uint8_t v) { b[o] = v; }
static void w16(volatile uint8_t *b, uint32_t o, uint16_t v) { *(volatile uint16_t *)(b + o) = v; }
static void w32(volatile uint8_t *b, uint32_t o, uint32_t v) { *(volatile uint32_t *)(b + o) = v; }
static void w64(volatile uint8_t *b, uint32_t o, uint64_t v) { w32(b, o, (uint32_t)v); w32(b, o + 4, (uint32_t)(v >> 32)); }

static void sleep_us(long us) {
    struct timespec ts = { us / 1000000, (us % 1000000) * 1000 };
    nanosleep(&ts, 0);
}

static int fail(const char *what) {
    fprintf(stderr, "virtio-blk %s: %s\n", where, what);
    if (common) w8(common, C_STATUS, r8(common, C_STATUS) | S_FAILED);
    return 1;
}

// Map the structure a virtio capability points at
static volatile uint8_t *cap_target(uint8_t cap) {
    static volatile uint8_t *bars[6];
    uint8_t bar = (uint8_t)rx_drv_cfg_read(dev, cap + 4, 1);
    uint32_t off = rx_drv_cfg_read(dev, cap + 8, 4);
    if (bar > 5) return 0;
    if (!bars[bar]) bars[bar] = rx_drv_map_bar(dev, bar, 0);
    return bars[bar] ? bars[bar] + off : 0;
}

// One request: a header, maybe data, a status byte. 0 or -errno.
static int request(uint32_t type, uint64_t sector, uint32_t bytes, int to_device) {
    w32(hdr, 0, type);
    w32(hdr, 4, 0);
    w64(hdr, 8, sector);
    *status = 0xFF;
    int n = 0;
    descs[n] = (struct desc){ hdr_pa, 16, VRING_DESC_F_NEXT, 1 };
    n++;
    if (bytes) {
        descs[n] = (struct desc){ data.pa, bytes, (uint16_t)(VRING_DESC_F_NEXT | (to_device ? 0 : VRING_DESC_F_WRITE)), 2 };
        n++;
    }
    descs[n] = (struct desc){ status_pa, 1, VRING_DESC_F_WRITE, 0 };
    if (bytes == 0) descs[0].next = 1;
    uint16_t idx = avail[1];
    avail[2 + idx % qsize] = 0;  // the chain starts at descriptor 0
    rx_dma_barrier();
    avail[1] = (uint16_t)(idx + 1);
    rx_dma_barrier();
    w16(notify_at, 0, 0);        // queue 0
    // Wait for the used ring to move: by interrupt, or by polling
    for (int waited = 0; used_hdr[1] == last_used; ) {
        if (polling) {
            sleep_us(waited < 100 ? 10 : 1000);
            waited++;
            if (waited > 100 + 10000) return -ETIMEDOUT; // ~10 s
        } else {
            int r = rx_drv_wait_irq(irq, 10000);
            if (r <= 0 && used_hdr[1] == last_used) {
                fprintf(stderr, "virtio-blk %s: no interrupt for 10 s; polling from now on\n", where);
                polling = 1;
            }
        }
    }
    rx_dma_barrier();
    last_used++;
    uint8_t st = *status;
    return st == 0 ? 0 : st == 2 ? -ENOTSUP : -EIO;
}

static int rd(void *ctx, uint64_t block, uint32_t count, void *buf) {
    (void)ctx;
    int r = request(T_IN, block, count * 512u, 0);
    if (r == 0) memcpy(buf, (const void *)data.va, (size_t)count * 512u);
    return r;
}

static int wr(void *ctx, uint64_t block, uint32_t count, const void *buf) {
    (void)ctx;
    memcpy((void *)data.va, buf, (size_t)count * 512u);
    return request(T_OUT, block, count * 512u, 1);
}

static int has_flush;

static int fl(void *ctx) {
    (void)ctx;
    return has_flush ? request(T_FLUSH, 0, 0, 0) : 0;
}

int main(int argc, char **argv) {
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "virtio-blk: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    // Find the configuration structures
    uint8_t cap = 0, notify_cap = 0;
    volatile uint8_t *notify_base = 0;
    uint32_t notify_mult = 0;
    while ((cap = rx_drv_find_cap(dev, 0x09, cap))) {
        uint8_t type = (uint8_t)rx_drv_cfg_read(dev, cap + 3, 1);
        if (type == CAP_COMMON && !common) common = cap_target(cap);
        else if (type == CAP_NOTIFY && !notify_base) {
            notify_base = cap_target(cap);
            notify_mult = rx_drv_cfg_read(dev, cap + 16, 4);
            notify_cap = cap;
        } else if (type == CAP_DEVICE && !devcfg) devcfg = cap_target(cap);
    }
    if (!common || !notify_base || !devcfg) return fail("no virtio 1.0 configuration structures (a legacy-only device?)");
    (void)notify_cap;

    // Reset, then acknowledge
    w8(common, C_STATUS, 0);
    for (int k = 0; k < 1000 && r8(common, C_STATUS); k++) sleep_us(100);
    w8(common, C_STATUS, S_ACK);
    w8(common, C_STATUS, S_ACK | S_DRIVER);
    w32(common, C_DFSELECT, 0);
    uint64_t f = r32(common, C_DF);
    w32(common, C_DFSELECT, 1);
    f |= (uint64_t)r32(common, C_DF) << 32;
    if (!(f & F_VERSION_1)) return fail("the device does not offer VIRTIO_F_VERSION_1");
    uint64_t want = f & (F_VERSION_1 | F_RO | F_BLK_SIZE | F_FLUSH);
    w32(common, C_GFSELECT, 0);
    w32(common, C_GF, (uint32_t)want);
    w32(common, C_GFSELECT, 1);
    w32(common, C_GF, (uint32_t)(want >> 32));
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK);
    if (!(r8(common, C_STATUS) & S_FEAT_OK)) return fail("the device refused our features");
    has_flush = (want & F_FLUSH) != 0;

    // Memory: the queue, then a header and a status byte; data separately
    rx_handle_t bti;
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    if (rx_dma_alloc(bti, 3 * 4096, 0, &ring) < 0 || rx_dma_alloc(bti, DATA_SIZE, 0, &data) < 0) return fail("no DMA memory");
    rx_drv_bus_master(dev, 1);

    // Interrupt: MSI-X vector 0 for the queue; without MSI-X, poll
    uint32_t used_mode = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSIX, 0, &irq, &used_mode) < 0) polling = 1;
    w16(common, C_MSIX_CFG, NO_VECTOR);

    w16(common, C_QSELECT, 0);
    uint16_t max = r16(common, C_QSIZE);
    if (!max) return fail("queue 0 does not exist");
    qsize = max < QSIZE ? max : QSIZE;
    w16(common, C_QSIZE, qsize);
    descs = (volatile struct desc *)ring.va;                  // 16 * qsize bytes
    avail = (volatile uint16_t *)(ring.va + 1024);            // 6 + 2 * qsize
    used_hdr = (volatile uint16_t *)(ring.va + 4096);         // 6 + 8 * qsize
    hdr = ring.va + 8192;
    status = ring.va + 8192 + 64;
    hdr_pa = ring.pa + 8192;
    status_pa = ring.pa + 8192 + 64;
    w64(common, C_QDESC, ring.pa);
    w64(common, C_QDRIVER, ring.pa + 1024);
    w64(common, C_QDEVICE, ring.pa + 4096);
    if (!polling) {
        w16(common, C_QMSIX, 0);
        if (r16(common, C_QMSIX) != 0) {
            fprintf(stderr, "virtio-blk %s: the device did not take MSI-X vector 0; polling\n", where);
            polling = 1;
        }
    }
    notify_at = notify_base + (uint32_t)r16(common, C_QNOTIFY) * notify_mult;
    w16(common, C_QENABLE, 1);
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK | S_DRIVER_OK);

    // Capacity (in 512-byte sectors, whatever the block size), read
    // until the configuration generation is stable
    uint64_t sectors;
    uint8_t gen;
    do {
        gen = r8(common, C_GEN);
        sectors = r32(devcfg, 0) | (uint64_t)r32(devcfg, 4) << 32;
    } while (gen != r8(common, C_GEN));

    rx_block_info_t info = { 0 };
    strcpy(info.name, "vd@");
    info.block_count = sectors;
    info.block_size = 512;
    info.flags = (want & F_RO) ? RX_BLOCK_FLAG_READONLY : 0;
    strcpy(info.driver, "virtio-blk");
    // The serial number, if the device has one
    memset((void *)data.va, 0, 32);
    if (request(T_GET_ID, 0, 20, 0) == 0 && data.va[0]) snprintf(info.model, sizeof info.model, "VirtIO %.20s", (const char *)data.va);
    else strcpy(info.model, "VirtIO disk");
    rx_blockdev_ops_t ops = { rd, (want & F_RO) ? 0 : wr, fl };
    int r = rx_blockdev_publish(&ops, 0, &info, DATA_SIZE / 512, 0);
    if (r < 0) {
        fprintf(stderr, "virtio-blk %s: cannot publish the disk: %s\n", where, strerror(-r));
        return 1;
    }
    return rx_blockdev_run();
}
