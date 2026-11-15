// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// e1000e: Intel's PCI Express gigabit network cards (network step D4,
// docs/NETWORK.md): the 82574L (QEMU's -device e1000e) and, written from
// the same register set, the I217/I218/I219 of laptops (untested on
// hardware yet). Like virtio-net, it moves Ethernet frames between the
// card and netd over /svc/netdev (<rustux/netdev.h>) and nothing more.
// devmgr starts it for the IDs in /etc/drivers.
//
//   e1000e BDF
//
// Legacy descriptors: a receive ring and a transmit ring of 256 each,
// 2 KiB buffers; no offloads (netd computes the checksums). MSI on a
// thread that drains the receive ring (polling without it); the link
// state from STATUS.LU, on the link-change interrupt and every second.
// Datasheets: "Intel 82574 GbE Controller Family" (registers 10.2),
// "Intel Ethernet Connection I219" (the same MAC registers).

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

#define CTRL     0x0000
#define STATUS   0x0008
#define EERD     0x0014
#define CTRL_EXT 0x0018
#define ICR      0x00C0
#define IMS      0x00D0
#define IMC      0x00D8
#define RCTL     0x0100
#define TCTL     0x0400
#define TIPG     0x0410
#define RDBAL    0x2800
#define RDBAH    0x2804
#define RDLEN    0x2808
#define RDH      0x2810
#define RDT      0x2818
#define RDTR     0x2820
#define TDBAL    0x3800
#define TDBAH    0x3804
#define TDLEN    0x3808
#define TDH      0x3810
#define TDT      0x3818
#define TXDCTL   0x3828
#define MTA      0x5200
#define RAL0     0x5400
#define RAH0     0x5404
#define FWSM     0x5B54

#define CTRL_SLU     (1u << 6)
#define CTRL_FRCSPD  (1u << 11)
#define CTRL_FRCDPX  (1u << 12)
#define CTRL_RST     (1u << 26)
#define CTRL_PHY_RST (1u << 31)
#define STATUS_LU    (1u << 1)
#define RCTL_EN      (1u << 1)
#define RCTL_UPE     (1u << 3)
#define RCTL_MPE     (1u << 4)
#define RCTL_BAM     (1u << 15)
#define RCTL_SECRC   (1u << 26)
#define TCTL_EN      (1u << 1)
#define TCTL_PSP     (1u << 3)
#define ICR_TXDW     (1u << 0)
#define ICR_LSC      (1u << 2)
#define ICR_RXDMT0   (1u << 4)
#define ICR_RXO      (1u << 6)
#define ICR_RXT0     (1u << 7)

#define NRX   256
#define NTX   256
#define BUFSZ 2048

struct rxd { uint64_t addr; uint16_t len, csum; uint8_t status, errors; uint16_t special; };
struct txd { uint64_t addr; uint16_t len; uint8_t cso, cmd, status, css; uint16_t special; };

static const char *where = "?";
static rx_handle_t dev, irq, bti;
static volatile uint8_t *regs;
static int polling;
static rx_dma_buf_t rx_ring, tx_ring, rx_bufs, tx_bufs;
static volatile struct rxd *rxr;
static volatile struct txd *txr;
static uint32_t rx_next, tx_next, tx_clean;
static uint8_t mac[6];
static int link_up;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t netd;
static uint16_t device_id;

static uint32_t rd(uint32_t o) { return *(volatile uint32_t *)(regs + o); }
static void wr(uint32_t o, uint32_t v) { *(volatile uint32_t *)(regs + o) = v; }

static void sleep_us(long us) {
    struct timespec ts = { us / 1000000, (us % 1000000) * 1000 };
    nanosleep(&ts, 0);
}

static int fail(const char *what) {
    fprintf(stderr, "e1000e %s: %s\n", where, what);
    return 1;
}

// ---- netd ----

static void send_netd(uint32_t op, uint32_t a, const void *data, uint32_t len) {
    char buf[sizeof(rx_netdev_msg_t) + sizeof(rx_netdev_hello_t) + RX_NETDEV_MAX_FRAME];
    rx_netdev_msg_t m = { op, a };
    if (len > sizeof buf - sizeof m) return;
    memcpy(buf, &m, sizeof m);
    if (len) memcpy(buf + sizeof m, data, len);
    rx_handle_t ch = __atomic_load_n(&netd, __ATOMIC_ACQUIRE);
    if (ch) rx_channel_write(ch, 0, buf, (uint32_t)(sizeof m + len), 0, 0);
}

static void hello(void) {
    rx_netdev_hello_t h;
    memset(&h, 0, sizeof h);
    memcpy(h.mac, mac, 6);
    h.link = (uint16_t)link_up;
    snprintf(h.driver, sizeof h.driver, "e1000e %s", where);
    send_netd(RX_NETDEV_HELLO, 1500, &h, sizeof h);
}

// ---- the card ----

// A word of the NVM (EEPROM or flash) through EERD
static int nvm_word(uint32_t addr, uint16_t *out) {
    wr(EERD, (addr << 2) | 1);
    for (int k = 0; k < 10000; k++) {
        uint32_t v = rd(EERD);
        if (v & 2) {
            *out = (uint16_t)(v >> 16);
            return 0;
        }
        sleep_us(5);
    }
    return -1;
}

static void read_mac(void) {
    uint32_t lo = rd(RAL0), hi = rd(RAH0);
    if (hi & (1u << 31)) {
        for (int i = 0; i < 4; i++) mac[i] = (uint8_t)(lo >> (8 * i));
        mac[4] = (uint8_t)hi;
        mac[5] = (uint8_t)(hi >> 8);
        return;
    }
    uint16_t w[3];
    if (nvm_word(0, &w[0]) == 0 && nvm_word(1, &w[1]) == 0 && nvm_word(2, &w[2]) == 0) {
        for (int i = 0; i < 3; i++) {
            mac[2 * i] = (uint8_t)w[i];
            mac[2 * i + 1] = (uint8_t)(w[i] >> 8);
        }
    } else {
        // A locally administered address of our own
        uint64_t t = rx_clock_get_monotonic();
        uint8_t m[6] = { 0x02, 0x52, 0x58, (uint8_t)(t >> 16), (uint8_t)(t >> 8), (uint8_t)t };
        memcpy(mac, m, 6);
    }
    wr(RAL0, (uint32_t)mac[0] | (uint32_t)mac[1] << 8 | (uint32_t)mac[2] << 16 | (uint32_t)mac[3] << 24);
    wr(RAH0, (uint32_t)mac[4] | (uint32_t)mac[5] << 8 | (1u << 31));
}

static int reset(void) {
    wr(IMC, 0xFFFFFFFFu);
    rd(ICR);
    wr(RCTL, 0);
    wr(TCTL, TCTL_PSP);
    sleep_us(10000);
    wr(CTRL, rd(CTRL) | CTRL_RST);
    sleep_us(20000);
    for (int k = 0; k < 1000 && (rd(CTRL) & CTRL_RST); k++) sleep_us(100);
    if (rd(CTRL) & CTRL_RST) return -1;
    wr(IMC, 0xFFFFFFFFu);
    rd(ICR);
    // Link up, speed and duplex from auto-negotiation; the PHY out of reset
    uint32_t c = rd(CTRL);
    c |= CTRL_SLU;
    c &= ~(CTRL_FRCSPD | CTRL_FRCDPX | CTRL_PHY_RST);
    wr(CTRL, c);
    return 0;
}

static int rings(void) {
    if (rx_dma_alloc(bti, NRX * sizeof(struct rxd), 0, &rx_ring) < 0 || rx_dma_alloc(bti, NTX * sizeof(struct txd), 0, &tx_ring) < 0 ||
        rx_dma_alloc(bti, (uint64_t)NRX * BUFSZ, 0, &rx_bufs) < 0 || rx_dma_alloc(bti, (uint64_t)NTX * BUFSZ, 0, &tx_bufs) < 0)
        return -1;
    rxr = (volatile struct rxd *)rx_ring.va;
    txr = (volatile struct txd *)tx_ring.va;
    memset((void *)rxr, 0, NRX * sizeof(struct rxd));
    memset((void *)txr, 0, NTX * sizeof(struct txd));
    for (uint32_t i = 0; i < NRX; i++) rxr[i].addr = rx_bufs.pa + (uint64_t)i * BUFSZ;
    for (uint32_t i = 0; i < NTX; i++) {
        txr[i].addr = tx_bufs.pa + (uint64_t)i * BUFSZ;
        txr[i].status = 1; // done: free
    }
    // Every multicast address: IPv6's neighbour discovery needs them
    for (uint32_t i = 0; i < 128; i++) wr(MTA + 4 * i, 0);
    wr(RDBAL, (uint32_t)rx_ring.pa);
    wr(RDBAH, (uint32_t)(rx_ring.pa >> 32));
    wr(RDLEN, NRX * sizeof(struct rxd));
    wr(RDH, 0);
    wr(RDT, NRX - 1);
    rx_next = 0;
    wr(RDTR, 0);
    wr(RCTL, RCTL_EN | RCTL_BAM | RCTL_MPE | RCTL_SECRC); // 2 KiB buffers (BSIZE 0)
    wr(TDBAL, (uint32_t)tx_ring.pa);
    wr(TDBAH, (uint32_t)(tx_ring.pa >> 32));
    wr(TDLEN, NTX * sizeof(struct txd));
    wr(TDH, 0);
    wr(TDT, 0);
    tx_next = tx_clean = 0;
    wr(TIPG, 10 | (8 << 10) | (6 << 20));
    wr(TCTL, TCTL_EN | TCTL_PSP | (0x0F << 4) | (0x3F << 12));
    return 0;
}

// Frames the card received, to netd; descriptors back to the card
static void rx_drain(void) {
    int got = 0;
    for (;;) {
        volatile struct rxd *d = &rxr[rx_next];
        if (!(d->status & 1)) break;
        rx_dma_barrier();
        uint16_t len = d->len;
        // A whole frame in one buffer (EOP), without errors
        if ((d->status & 2) && !d->errors && len > 0 && len <= RX_NETDEV_MAX_FRAME)
            send_netd(RX_NETDEV_FRAME, 0, (const void *)(rx_bufs.va + (uint64_t)rx_next * BUFSZ), len);
        d->status = 0;
        rx_dma_barrier();
        wr(RDT, rx_next);
        rx_next = (rx_next + 1) % NRX;
        got = 1;
    }
    (void)got;
}

static void transmit(const uint8_t *frame, uint32_t len) {
    if (len > BUFSZ || len < 14) return;
    pthread_mutex_lock(&lock);
    volatile struct txd *d = &txr[tx_next];
    if (!(d->status & 1)) {
        pthread_mutex_unlock(&lock);
        return; // the ring is full: dropped (TCP sends again)
    }
    memcpy((void *)(tx_bufs.va + (uint64_t)tx_next * BUFSZ), frame, len);
    d->len = (uint16_t)(len < 60 ? 60 : len); // the card pads to the minimum with PSP; the length must still cover it
    if (len < 60) memset((void *)(tx_bufs.va + (uint64_t)tx_next * BUFSZ + len), 0, 60 - len);
    d->cso = 0;
    d->css = 0;
    d->special = 0;
    d->status = 0;
    d->cmd = 0x01 | 0x02 | 0x08; // EOP, IFCS (append the FCS), RS (report status)
    tx_next = (tx_next + 1) % NTX;
    rx_dma_barrier();
    wr(TDT, tx_next);
    pthread_mutex_unlock(&lock);
}

static void check_link(void) {
    int up = (rd(STATUS) & STATUS_LU) != 0;
    if (up != link_up) {
        link_up = up;
        printf("e1000e %s: link %s\n", where, up ? "up" : "down");
        send_netd(RX_NETDEV_LINK, (uint32_t)up, 0, 0);
    }
}

static void *irq_thread(void *arg) {
    (void)arg;
    uint64_t last = 0;
    for (;;) {
        if (polling) sleep_us(1000);
        else rx_drv_wait_irq(irq, 1000);
        uint32_t cause = rd(ICR); // reading clears it
        pthread_mutex_lock(&lock);
        rx_drain();
        pthread_mutex_unlock(&lock);
        uint64_t now = rx_clock_get_monotonic();
        if ((cause & ICR_LSC) || now - last > 1000000000ull) {
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
        fprintf(stderr, "e1000e: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    device_id = (uint16_t)(rx_drv_cfg_read(dev, 0, 4) >> 16);
    regs = rx_drv_map_bar(dev, 0, 0);
    if (!regs) return fail("cannot map BAR 0");
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    rx_drv_bus_master(dev, 1);
    if (reset() < 0) return fail("the card did not come out of reset");
    read_mac();
    if (rings() < 0) return fail("cannot allocate the rings");
    uint32_t used = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSI, 0, &irq, &used) < 0) polling = 1;
    wr(IMS, ICR_LSC | ICR_RXT0 | ICR_RXDMT0 | ICR_RXO | ICR_TXDW);
    link_up = (rd(STATUS) & STATUS_LU) != 0;
    printf("e1000e %s: %04x, %02x:%02x:%02x:%02x:%02x:%02x, link %s%s\n", where, device_id, mac[0], mac[1], mac[2], mac[3], mac[4],
           mac[5], link_up ? "up" : "down", polling ? " (polling)" : "");

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
                rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, 0);
                continue;
            }
            if (s != RX_OK) break;
            if (nh) rx_handle_close(h);
            rx_netdev_msg_t m;
            if (nb < sizeof m) continue;
            memcpy(&m, buf, sizeof m);
            if (m.op == RX_NETDEV_FRAME) transmit((const uint8_t *)buf + sizeof m, nb - (uint32_t)sizeof m);
        }
        pthread_mutex_lock(&lock);
        __atomic_store_n(&netd, 0, __ATOMIC_RELEASE);
        pthread_mutex_unlock(&lock);
        rx_handle_close(ch);
        printf("e1000e %s: netd is gone; waiting for it\n", where);
    }
}
