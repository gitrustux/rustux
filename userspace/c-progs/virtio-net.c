// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// virtio-net: the network card of QEMU, KVM and Proxmox (NETWORK.md step
// D1, docs/NETWORK.md). It moves Ethernet frames between the device and
// the network service, netd, over /svc/netdev (<rustux/netdev.h>); the
// TCP/IP stack is netd's. devmgr starts it for 1af4:1000 and 1af4:1041.
//
//   virtio-net BDF
//
// No offloads (checksums, segmentation): netd does it all. One receive
// queue of 64 buffers, one transmit queue of 64; MSI-X vector 0 for both
// (polling without MSI-X). Spec: "Virtual I/O Device (VIRTIO) Version
// 1.2", sections 4.1 and 5.1.

#include <errno.h>
#include <pthread.h>
#include <rustux/driver.h>
#include <rustux/netdev.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define CAP_COMMON 1
#define CAP_NOTIFY 2
#define CAP_DEVICE 4

#define C_DFSELECT  0x00
#define C_DF        0x04
#define C_GFSELECT  0x08
#define C_GF        0x0C
#define C_MSIX_CFG  0x10
#define C_STATUS    0x14
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
#define F_MAC       (1ull << 5)
#define F_STATUS    (1ull << 16)
#define F_VERSION_1 (1ull << 32)
#define NO_VECTOR   0xFFFF
#define D_WRITE     2

#define QN      64      // buffers in each queue
#define BUFSZ   2048    // a frame and the virtio-net header
#define HDR     12      // struct virtio_net_hdr with VERSION_1

struct desc { uint64_t addr; uint32_t len; uint16_t flags, next; };

struct vq {
    volatile struct desc *desc;
    volatile uint16_t *avail;
    volatile uint16_t *used;
    uint16_t size, last_used;
    volatile uint8_t *notify;
    uint16_t index;
    rx_dma_buf_t ring, bufs;  // the ring; QN buffers of BUFSZ
};

static const char *where = "?";
static rx_handle_t dev, irq, bti;
static volatile uint8_t *common, *notify_base, *devcfg;
static uint32_t notify_mult;
static int polling;
static struct vq rxq, txq;
static uint8_t mac[6];
static uint16_t link_up = 1;
static int has_status;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t netd;        // the channel to netd, 0 while it is not there (atomic: read
                                // by send_netd, which runs with or without the lock)
static uint16_t tx_free[QN], ntx_free;

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
    fprintf(stderr, "virtio-net %s: %s\n", where, what);
    if (common) w8(common, C_STATUS, r8(common, C_STATUS) | S_FAILED);
    return 1;
}

static volatile uint8_t *cap_target(uint8_t cap) {
    static volatile uint8_t *bars[6];
    uint8_t bar = (uint8_t)rx_drv_cfg_read(dev, cap + 4, 1);
    uint32_t off = rx_drv_cfg_read(dev, cap + 8, 4);
    if (bar > 5) return 0;
    if (!bars[bar]) bars[bar] = rx_drv_map_bar(dev, bar, 0);
    return bars[bar] ? bars[bar] + off : 0;
}

static int vq_setup(uint16_t index, struct vq *q) {
    w16(common, C_QSELECT, index);
    uint16_t max = r16(common, C_QSIZE);
    if (!max) return -1;
    q->size = max < QN ? max : QN;
    q->index = index;
    w16(common, C_QSIZE, q->size);
    if (rx_dma_alloc(bti, 3 * 4096, 0, &q->ring) < 0 || rx_dma_alloc(bti, (uint64_t)QN * BUFSZ, 0, &q->bufs) < 0) return -1;
    memset((void *)q->ring.va, 0, 3 * 4096);
    q->desc = (volatile struct desc *)q->ring.va;
    q->avail = (volatile uint16_t *)(q->ring.va + 2048);
    q->used = (volatile uint16_t *)(q->ring.va + 4096);
    w64(common, C_QDESC, q->ring.pa);
    w64(common, C_QDRIVER, q->ring.pa + 2048);
    w64(common, C_QDEVICE, q->ring.pa + 4096);
    if (!polling) {
        w16(common, C_QMSIX, 0);
        if (r16(common, C_QMSIX) != 0) polling = 1;
    }
    q->notify = notify_base + (uint32_t)r16(common, C_QNOTIFY) * notify_mult;
    w16(common, C_QENABLE, 1);
    return 0;
}

static void kick(struct vq *q) {
    rx_dma_barrier();
    w16(q->notify, 0, q->index);
}

// Give receive buffer i to the device
static void rx_post(uint16_t i) {
    rxq.desc[i] = (struct desc){ rxq.bufs.pa + (uint64_t)i * BUFSZ, BUFSZ, D_WRITE, 0 };
    uint16_t idx = rxq.avail[1];
    rxq.avail[2 + idx % rxq.size] = i;
    rx_dma_barrier();
    rxq.avail[1] = (uint16_t)(idx + 1);
}

// ---- netd ----

static void send_netd(uint32_t op, uint32_t a, const void *data, uint32_t len) {
    char buf[sizeof(rx_netdev_msg_t) + sizeof(rx_netdev_hello_t) + RX_NETDEV_MAX_FRAME];
    rx_netdev_msg_t m = { op, a };
    if (len > sizeof buf - sizeof m) return;
    memcpy(buf, &m, sizeof m);
    if (len) memcpy(buf + sizeof m, data, len);
    rx_handle_t ch = __atomic_load_n(&netd, __ATOMIC_ACQUIRE);
    if (ch) rx_channel_write(ch, 0, buf, (uint32_t)(sizeof m + len), 0, 0); // dropped if netd is behind
}

static void hello(void) {
    rx_netdev_hello_t h;
    memset(&h, 0, sizeof h);
    memcpy(h.mac, mac, 6);
    h.link = link_up;
    snprintf(h.driver, sizeof h.driver, "virtio-net %s", where);
    send_netd(RX_NETDEV_HELLO, 1500, &h, sizeof h);
}

// Frames the device received, to netd; buffers back to the device
static void rx_drain(void) {
    int got = 0;
    while (rxq.used[1] != rxq.last_used) {
        rx_dma_barrier();
        uint16_t slot = rxq.last_used % rxq.size;
        uint32_t id = *(volatile uint32_t *)(rxq.used + 2 + slot * 4);
        uint32_t len = *(volatile uint32_t *)(rxq.used + 2 + slot * 4 + 2);
        rxq.last_used++;
        if (id >= rxq.size) continue;
        if (len > HDR) send_netd(RX_NETDEV_FRAME, 0, (const void *)(rxq.bufs.va + (uint64_t)id * BUFSZ + HDR), len - HDR);
        rx_post((uint16_t)id);
        got = 1;
    }
    if (got) kick(&rxq);
}

// Transmit buffers the device has finished with
static void tx_reclaim(void) {
    while (txq.used[1] != txq.last_used) {
        rx_dma_barrier();
        uint16_t slot = txq.last_used % txq.size;
        uint32_t id = *(volatile uint32_t *)(txq.used + 2 + slot * 4);
        txq.last_used++;
        if (id < txq.size) tx_free[ntx_free++] = (uint16_t)id;
    }
}

static void transmit(const uint8_t *frame, uint32_t len) {
    if (len > BUFSZ - HDR) return;
    pthread_mutex_lock(&lock);
    tx_reclaim();
    if (!ntx_free) {
        pthread_mutex_unlock(&lock);
        return; // the queue is full: dropped (TCP sends again)
    }
    uint16_t i = tx_free[--ntx_free];
    uint8_t *b = (uint8_t *)(txq.bufs.va + (uint64_t)i * BUFSZ);
    memset(b, 0, HDR);
    memcpy(b + HDR, frame, len);
    txq.desc[i] = (struct desc){ txq.bufs.pa + (uint64_t)i * BUFSZ, HDR + len, 0, 0 };
    uint16_t idx = txq.avail[1];
    txq.avail[2 + idx % txq.size] = i;
    rx_dma_barrier();
    txq.avail[1] = (uint16_t)(idx + 1);
    kick(&txq);
    pthread_mutex_unlock(&lock);
}

static void check_link(void) {
    if (!has_status) return;
    uint16_t up = r16(devcfg, 6) & 1;
    if (up != link_up) {
        link_up = up;
        printf("virtio-net %s: link %s\n", where, up ? "up" : "down");
        send_netd(RX_NETDEV_LINK, up, 0, 0);
    }
}

// Interrupts: received frames (and link changes, looked at every second)
static void *irq_thread(void *arg) {
    (void)arg;
    for (;;) {
        if (polling) sleep_us(1000);
        else rx_drv_wait_irq(irq, 1000);
        pthread_mutex_lock(&lock);
        rx_drain();
        pthread_mutex_unlock(&lock);
        static uint64_t last;
        uint64_t now = rx_clock_get_monotonic();
        if (now - last > 1000000000ull) {
            last = now;
            check_link();
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    rx_malloc_commit(1);
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "virtio-net: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    uint8_t cap = 0;
    while ((cap = rx_drv_find_cap(dev, 0x09, cap))) {
        uint8_t type = (uint8_t)rx_drv_cfg_read(dev, cap + 3, 1);
        if (type == CAP_COMMON && !common) common = cap_target(cap);
        else if (type == CAP_NOTIFY && !notify_base) {
            notify_base = cap_target(cap);
            notify_mult = rx_drv_cfg_read(dev, cap + 16, 4);
        } else if (type == CAP_DEVICE && !devcfg) devcfg = cap_target(cap);
    }
    if (!common || !notify_base || !devcfg) return fail("no virtio 1.0 configuration structures");

    w8(common, C_STATUS, 0);
    for (int k = 0; k < 1000 && r8(common, C_STATUS); k++) sleep_us(100);
    w8(common, C_STATUS, S_ACK);
    w8(common, C_STATUS, S_ACK | S_DRIVER);
    w32(common, C_DFSELECT, 0);
    uint64_t f = r32(common, C_DF);
    w32(common, C_DFSELECT, 1);
    f |= (uint64_t)r32(common, C_DF) << 32;
    if (!(f & F_VERSION_1)) return fail("the device does not offer VIRTIO_F_VERSION_1");
    uint64_t want = F_VERSION_1 | (f & (F_MAC | F_STATUS));
    w32(common, C_GFSELECT, 0);
    w32(common, C_GF, (uint32_t)want);
    w32(common, C_GFSELECT, 1);
    w32(common, C_GF, (uint32_t)(want >> 32));
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK);
    if (!(r8(common, C_STATUS) & S_FEAT_OK)) return fail("the device refused our features");
    has_status = (want & F_STATUS) != 0;

    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    rx_drv_bus_master(dev, 1);
    uint32_t used_mode = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSIX, 0, &irq, &used_mode) < 0) polling = 1;
    w16(common, C_MSIX_CFG, NO_VECTOR);
    if (vq_setup(0, &rxq) < 0 || vq_setup(1, &txq) < 0) return fail("cannot set up the queues");
    w8(common, C_STATUS, S_ACK | S_DRIVER | S_FEAT_OK | S_DRIVER_OK);

    if (want & F_MAC) {
        for (int i = 0; i < 6; i++) mac[i] = r8(devcfg, (uint32_t)i);
    } else {
        // A locally administered address of our own
        uint64_t t = rx_clock_get_monotonic();
        uint8_t m[6] = { 0x02, 0x52, 0x58, (uint8_t)(t >> 16), (uint8_t)(t >> 8), (uint8_t)t };
        memcpy(mac, m, 6);
    }
    if (has_status) link_up = r16(devcfg, 6) & 1;
    for (uint16_t i = 0; i < rxq.size; i++) rx_post(i);
    kick(&rxq);
    for (uint16_t i = 0; i < txq.size; i++) tx_free[ntx_free++] = i;
    printf("virtio-net %s: %02x:%02x:%02x:%02x:%02x:%02x, link %s%s\n", where, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
           link_up ? "up" : "down", polling ? " (polling)" : "");

    pthread_t t;
    if (pthread_create(&t, 0, irq_thread, 0) != 0) return fail("no thread");

    // netd: connect (and again if it goes away); frames to send from it
    static char buf[sizeof(rx_netdev_msg_t) + RX_NETDEV_MAX_FRAME + 64];
    for (;;) {
        rx_handle_t ch;
        if (rx_service_connect("/svc/netdev", &ch) < 0) {
            sleep_us(500000);
            continue;
        }
        __atomic_store_n(&netd, ch, __ATOMIC_RELEASE);
        hello();
        for (;;) {
            uint32_t nb = 0, nh = 0;
            rx_handle_t h;
            rx_status_t s = rx_channel_read(ch, 0, buf, &h, sizeof buf, 1, &nb, &nh);
            if (s == RX_ERR_SHOULD_WAIT) {
                uint32_t pend = 0;
                rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
                continue;
            }
            if (s != RX_OK) break;
            if (nh) rx_handle_close(h);
            rx_netdev_msg_t m;
            if (nb < sizeof m) continue;
            memcpy(&m, buf, sizeof m);
            if (m.op == RX_NETDEV_FRAME) transmit((const uint8_t *)buf + sizeof m, nb - (uint32_t)sizeof m);
        }
        // Under the lock, so the receive thread is not writing to it
        pthread_mutex_lock(&lock);
        __atomic_store_n(&netd, 0, __ATOMIC_RELEASE);
        pthread_mutex_unlock(&lock);
        rx_handle_close(ch);
        printf("virtio-net %s: netd is gone; waiting for it\n", where);
    }
}
