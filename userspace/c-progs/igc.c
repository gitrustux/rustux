// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// igc: Intel's 2.5 GbE cards, the I225 and I226 (the laptop's Killer
// E3100, 8086:3100, is an I225; many AM5 and Intel desktop boards have an
// I225-V or I226-V), and the gigabit 82576 they descend from, which is
// QEMU's -device igb and so how this driver is tested (2026-11-25,
// CHECKLIST.md "Hardware session"). Like e1000e and virtio-net, it moves
// Ethernet frames between the card and netd over /svc/netdev
// (<rustux/netdev.h>) and nothing more. devmgr starts it for the IDs in
// /etc/drivers.
//
//   igc BDF
//
// Queue 0 only, with the advanced descriptors both families share: a
// receive ring and a transmit ring of 256, 2 KiB buffers, no offloads
// (netd computes the checksums). The two families differ in the reset
// (I225: device reset with the bus master off, then the NVM's
// auto-read) and in where the interrupt cause registers are (I225 0x1500,
// 82576 0x00C0). The PHY is powered up and told to auto-negotiate through
// MDIC. MSI wakes the receive thread, which
// also looks every millisecond (so a wrong interrupt setup costs speed,
// not the network); the link state from STATUS.LU.
// Datasheets: "Intel Ethernet Controller I225" (registers 8), "Intel
// 82576EB Gigabit Ethernet Controller" (registers 8); Linux's igc and igb.

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
#define EECD     0x0010
#define CTRL_EXT 0x0018
#define MDIC     0x0020
#define RCTL     0x0100
#define TCTL     0x0400
#define MTA      0x5200
#define RAL0     0x5400
#define RAH0     0x5404
#define RDBAL    0xC000
#define RDBAH    0xC004
#define RDLEN    0xC008
#define SRRCTL   0xC00C
#define RDH      0xC010
#define RDT      0xC018
#define RXDCTL   0xC028
#define TDBAL    0xE000
#define TDBAH    0xE004
#define TDLEN    0xE008
#define TDH      0xE010
#define TDT      0xE018
#define TXDCTL   0xE028

#define CTRL_GIO_MASTER_DISABLE (1u << 2)
#define CTRL_SLU         (1u << 6)
#define CTRL_FRCSPD      (1u << 11)
#define CTRL_FRCDPX      (1u << 12)
#define CTRL_RST         (1u << 26)
#define CTRL_DEV_RST     (1u << 29)
#define CTRL_EXT_DRV_LOAD (1u << 28)
#define STATUS_LU        (1u << 1)
#define STATUS_GIO_MASTER (1u << 19)
#define STATUS_2500      (1u << 22)
#define EECD_AUTO_RD     (1u << 9)
#define RCTL_EN          (1u << 1)
#define RCTL_MPE         (1u << 4)
#define RCTL_BAM         (1u << 15)
#define RCTL_SECRC       (1u << 26)
#define TCTL_EN          (1u << 1)
#define TCTL_PSP         (1u << 3)
#define QUEUE_ENABLE     (1u << 25)
#define SRRCTL_ADV_ONEBUF (1u << 25)
#define INT_TXDW         (1u << 0)
#define INT_LSC          (1u << 2)
#define INT_RXDMT0       (1u << 4)
#define INT_RXO          (1u << 6)
#define INT_RXT0         (1u << 7)

#define NRX   256
#define NTX   256
#define BUFSZ 2048

// Advanced descriptors: read format (what we give) and write-back (what
// the card returns in the same 16 bytes)
struct rxd { uint64_t addr, hdr; };          // wb: dword 2 status/errors, word 6 length
struct txd { uint64_t addr; uint32_t cmd_type_len, olinfo; }; // wb: olinfo bit 0 = done

static const char *where = "?";
static rx_handle_t dev, irq, bti;
static volatile uint8_t *regs;
static int polling, i225;
static uint32_t ICR, IMS, IMC;
static rx_dma_buf_t rx_ring, tx_ring, rx_bufs, tx_bufs;
static volatile struct rxd *rxr;
static volatile struct txd *txr;
static uint32_t rx_next, tx_next;
static uint8_t tx_busy[NTX];
static uint8_t mac[6];
static int link_up;
static int phy_addr = -1;        // the PHY that answered on MDIC

// The I225's PHY power management (as Linux's and the BSDs' igc): the
// firmware, or Windows going to sleep, can leave "go link disconnect" or
// the speed limits of low power set, and then no link ever comes
#define PHPM             0x0E14
#define PHPM_GO_LINKD    0x0020
#define PHPM_DIS_1000    0x0040
#define PHPM_RST_COMPL   0x0100
#define PHPM_ULP         0x0400
#define PHPM_DIS_2500    0x0800
#define CTRL_PHY_RST     (1u << 31)
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
    fprintf(stderr, "igc %s: %s\n", where, what);
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
    snprintf(h.driver, sizeof h.driver, "igc %s", where);
    send_netd(RX_NETDEV_HELLO, 1500, &h, sizeof h);
}

// ---- the card ----

// A register of the PHY (the I225's internal one, the 82576's) through MDIC (op 1 write, 2 read)
static int mdic(int phy, int reg, int op, uint16_t *val) {
    wr(MDIC, (uint32_t)(op == 1 ? *val : 0) | (uint32_t)reg << 16 | (uint32_t)phy << 21 | (uint32_t)op << 26);
    for (int k = 0; k < 2000; k++) {
        uint32_t v = rd(MDIC);
        if (v & (1u << 28)) {
            if (v & (1u << 30)) return -1;
            if (op == 2) *val = (uint16_t)v;
            return 0;
        }
        sleep_us(50);
    }
    return -1;
}

// A PHY register of MMD `dev` (clause 45) through the clause 22 MMD
// access registers 13 and 14 (op 1 write, 2 read)
static int mmd(int phy, int dev, int reg, int op, uint16_t *val) {
    uint16_t a = (uint16_t)dev, r = (uint16_t)reg, d = (uint16_t)(0x4000 | dev);
    if (mdic(phy, 13, 1, &a) < 0 || mdic(phy, 14, 1, &r) < 0 || mdic(phy, 13, 1, &d) < 0) return -1;
    return mdic(phy, 14, op, val);
}

// The PHY powered up and auto-negotiating everything it can (the
// firmware or the other system may have left it powered down or
// advertising less; QEMU's igb brings the link up on the restart)
static void phy_up(void) {
    for (int phy = 1; phy >= 0; phy--) {
        uint16_t id = 0, bmcr = 0;
        if (mdic(phy, 2, 2, &id) < 0 || id == 0 || id == 0xFFFF) continue;
        if (mdic(phy, 0, 2, &bmcr) < 0) continue;
        phy_addr = phy;
        if (i225) {
            // Advertise 10/100 half and full, 1000 full, 2500 full
            uint16_t an = 0, gt = 0, mg = 0;
            mdic(phy, 4, 2, &an);
            an = (uint16_t)((an & ~0x01E0u) | 0x01E0u | 0x0001u);
            mdic(phy, 4, 1, &an);
            mdic(phy, 9, 2, &gt);
            gt = (uint16_t)((gt & ~0x0300u) | 0x0200u);
            mdic(phy, 9, 1, &gt);
            if (mmd(phy, 7, 0x20, 2, &mg) == 0) {
                mg |= 0x0080;
                mmd(phy, 7, 0x20, 1, &mg);
            }
        }
        uint16_t nb = (uint16_t)((bmcr & ~(1u << 11)) | (1u << 12) | (1u << 9)); // not power down, auto-neg, restart
        mdic(phy, 0, 1, &nb);
        printf("igc %s: PHY %d (id %04x), control %04x -> %04x\n", where, phy, id, bmcr, nb);
        return;
    }
    printf("igc %s: no PHY answered on MDIC; leaving it as it is\n", where);
}

// What the card and its PHY say, for the log while there is no link
static void link_report(void) {
    uint16_t bmcr = 0, bmsr = 0, lpa = 0, gs = 0;
    if (phy_addr >= 0) {
        mdic(phy_addr, 0, 2, &bmcr);
        mdic(phy_addr, 1, 2, &bmsr);
        mdic(phy_addr, 1, 2, &bmsr); // latched: the second read is now
        mdic(phy_addr, 5, 2, &lpa);
        mdic(phy_addr, 10, 2, &gs);
    }
    printf("igc %s: no link yet: STATUS %08x CTRL %08x PHPM %08x; PHY %d BMCR %04x BMSR %04x partner %04x 1000T %04x\n", where,
           rd(STATUS), rd(CTRL), i225 ? rd(PHPM) : 0, phy_addr, bmcr, bmsr, lpa, gs);
}

static void read_mac(void) {
    uint32_t lo = rd(RAL0), hi = rd(RAH0);
    if ((hi & (1u << 31)) && (lo || (hi & 0xFFFF))) {
        for (int i = 0; i < 4; i++) mac[i] = (uint8_t)(lo >> (8 * i));
        mac[4] = (uint8_t)hi;
        mac[5] = (uint8_t)(hi >> 8);
        return;
    }
    // A locally administered address of our own (a card without its NVM)
    uint64_t t = rx_clock_get_monotonic();
    uint8_t m[6] = { 0x02, 0x52, 0x58, (uint8_t)(t >> 16), (uint8_t)(t >> 8), (uint8_t)t };
    memcpy(mac, m, 6);
    wr(RAL0, (uint32_t)mac[0] | (uint32_t)mac[1] << 8 | (uint32_t)mac[2] << 16 | (uint32_t)mac[3] << 24);
    wr(RAH0, (uint32_t)mac[4] | (uint32_t)mac[5] << 8 | (1u << 31));
}

static int reset(void) {
    wr(IMC, 0xFFFFFFFFu);
    rd(ICR);
    wr(RCTL, 0);
    wr(TCTL, TCTL_PSP);
    if (i225) {
        // Bus master off (no DMA in flight), then a device reset
        wr(CTRL, rd(CTRL) | CTRL_GIO_MASTER_DISABLE);
        for (int k = 0; k < 800 && (rd(STATUS) & STATUS_GIO_MASTER); k++) sleep_us(100);
        sleep_us(10000);
        wr(CTRL, rd(CTRL) | CTRL_DEV_RST);
    } else {
        sleep_us(10000);
        wr(CTRL, rd(CTRL) | CTRL_RST);
    }
    sleep_us(20000);
    for (int k = 0; k < 1000 && (rd(CTRL) & (CTRL_RST | CTRL_DEV_RST)); k++) sleep_us(100);
    if (rd(CTRL) & (CTRL_RST | CTRL_DEV_RST)) return -1;
    // The NVM read in again (the MAC address, the PHY's settings)
    for (int k = 0; k < 100 && !(rd(EECD) & EECD_AUTO_RD); k++) sleep_us(100);
    wr(IMC, 0xFFFFFFFFu);
    rd(ICR);
    wr(CTRL_EXT, rd(CTRL_EXT) | CTRL_EXT_DRV_LOAD);
    if (i225) {
        // The PHY reset (Linux's igc_phy_hw_reset), then its power
        // management back to full power and full speed
        uint32_t ph0 = rd(PHPM);
        wr(CTRL, rd(CTRL) | CTRL_PHY_RST);
        sleep_us(100);
        wr(CTRL, rd(CTRL) & ~CTRL_PHY_RST);
        for (int k = 0; k < 1000 && !(rd(PHPM) & PHPM_RST_COMPL); k++) sleep_us(100);
        sleep_us(100);
        uint32_t ph = rd(PHPM);
        wr(PHPM, ph & ~(uint32_t)(PHPM_GO_LINKD | PHPM_DIS_1000 | PHPM_DIS_2500 | PHPM_ULP));
        printf("igc %s: PHY power management %08x -> %08x\n", where, ph0, rd(PHPM));
    }
    // Link up, speed and duplex from auto-negotiation
    uint32_t c = rd(CTRL);
    c |= CTRL_SLU;
    c &= ~(CTRL_FRCSPD | CTRL_FRCDPX | CTRL_GIO_MASTER_DISABLE);
    wr(CTRL, c);
    phy_up();
    return 0;
}

static void rx_give(uint32_t i) {
    rxr[i].addr = rx_bufs.pa + (uint64_t)i * BUFSZ;
    rxr[i].hdr = 0; // clears the write-back's status (DD)
}

static int rings(void) {
    if (rx_dma_alloc(bti, NRX * sizeof(struct rxd), 0, &rx_ring) < 0 || rx_dma_alloc(bti, NTX * sizeof(struct txd), 0, &tx_ring) < 0 ||
        rx_dma_alloc(bti, (uint64_t)NRX * BUFSZ, 0, &rx_bufs) < 0 || rx_dma_alloc(bti, (uint64_t)NTX * BUFSZ, 0, &tx_bufs) < 0)
        return -1;
    rxr = (volatile struct rxd *)rx_ring.va;
    txr = (volatile struct txd *)tx_ring.va;
    memset((void *)txr, 0, NTX * sizeof(struct txd));
    for (uint32_t i = 0; i < NRX; i++) rx_give(i);
    // Every multicast address (MPE): IPv6's neighbour discovery needs them
    for (uint32_t i = 0; i < 128; i++) wr(MTA + 4 * i, 0);

    wr(RXDCTL, 0);
    wr(RDBAL, (uint32_t)rx_ring.pa);
    wr(RDBAH, (uint32_t)(rx_ring.pa >> 32));
    wr(RDLEN, NRX * sizeof(struct rxd));
    wr(SRRCTL, (BUFSZ >> 10) | SRRCTL_ADV_ONEBUF);
    wr(RDH, 0);
    wr(RDT, 0);
    wr(RXDCTL, QUEUE_ENABLE | (8 << 0) | (8 << 8) | (4 << 16)); // and the prefetch/host/write-back thresholds
    for (int k = 0; k < 100 && !(rd(RXDCTL) & QUEUE_ENABLE); k++) sleep_us(100);
    wr(RDT, NRX - 1);
    rx_next = 0;
    wr(RCTL, RCTL_EN | RCTL_BAM | RCTL_MPE | RCTL_SECRC);

    wr(TXDCTL, 0);
    wr(TDBAL, (uint32_t)tx_ring.pa);
    wr(TDBAH, (uint32_t)(tx_ring.pa >> 32));
    wr(TDLEN, NTX * sizeof(struct txd));
    wr(TDH, 0);
    wr(TDT, 0);
    wr(TXDCTL, QUEUE_ENABLE | (8 << 0) | (1 << 8) | (1 << 16));
    for (int k = 0; k < 100 && !(rd(TXDCTL) & QUEUE_ENABLE); k++) sleep_us(100);
    tx_next = 0;
    memset(tx_busy, 0, sizeof tx_busy);
    wr(TCTL, TCTL_EN | TCTL_PSP | (0x0F << 4) | (0x3F << 12));
    return 0;
}

// Frames the card received, to netd; descriptors back to the card
static void rx_drain(void) {
    for (;;) {
        volatile uint8_t *d = (volatile uint8_t *)&rxr[rx_next];
        uint32_t se = *(volatile uint32_t *)(d + 8);
        if (!(se & 1)) break; // DD
        rx_dma_barrier();
        uint16_t len = *(volatile uint16_t *)(d + 12);
        // A whole frame in one buffer (EOP), without a receive error (RXE)
        if ((se & 2) && !(se & 0x80000000u) && len > 0 && len <= RX_NETDEV_MAX_FRAME)
            send_netd(RX_NETDEV_FRAME, 0, (const void *)(rx_bufs.va + (uint64_t)rx_next * BUFSZ), len);
        rx_give(rx_next);
        rx_dma_barrier();
        wr(RDT, rx_next);
        rx_next = (rx_next + 1) % NRX;
    }
}

static void transmit(const uint8_t *frame, uint32_t len) {
    if (len > BUFSZ || len < 14) return;
    pthread_mutex_lock(&lock);
    volatile struct txd *d = &txr[tx_next];
    if (tx_busy[tx_next] && !(d->olinfo & 1)) {
        pthread_mutex_unlock(&lock);
        return; // the ring is full: dropped (TCP sends again)
    }
    uint8_t *b = (uint8_t *)(tx_bufs.va + (uint64_t)tx_next * BUFSZ);
    memcpy(b, frame, len);
    if (len < 60) {
        memset(b + len, 0, 60 - len);
        len = 60;
    }
    d->addr = tx_bufs.pa + (uint64_t)tx_next * BUFSZ;
    // DTYP data, DEXT, RS (report status), IFCS (append the FCS), EOP
    d->cmd_type_len = len | (3u << 20) | (1u << 29) | (1u << 27) | (1u << 25) | (1u << 24);
    d->olinfo = len << 14; // PAYLEN; clears the done bit
    tx_busy[tx_next] = 1;
    tx_next = (tx_next + 1) % NTX;
    rx_dma_barrier();
    wr(TDT, tx_next);
    pthread_mutex_unlock(&lock);
}

static void check_link(void) {
    uint32_t st = rd(STATUS);
    int up = (st & STATUS_LU) != 0;
    if (up != link_up) {
        link_up = up;
        const char *speed = (st & STATUS_2500) && i225 ? "2500" : ((st >> 6) & 3) == 2 ? "1000" : ((st >> 6) & 3) == 1 ? "100" : "10";
        if (up) printf("igc %s: link up, %s Mb/s %s duplex\n", where, speed, (st & 1) ? "full" : "half");
        else printf("igc %s: link down\n", where);
        send_netd(RX_NETDEV_LINK, (uint32_t)up, 0, 0);
    }
}

static void *irq_thread(void *arg) {
    (void)arg;
    uint64_t last = 0;
    for (;;) {
        if (polling) sleep_us(1000);
        else rx_drv_wait_irq(irq, 1);
        uint32_t cause = rd(ICR); // reading clears it
        pthread_mutex_lock(&lock);
        rx_drain();
        pthread_mutex_unlock(&lock);
        uint64_t now = rx_clock_get_monotonic();
        if ((cause & INT_LSC) || now - last > 1000000000ull) {
            last = now;
            check_link();
            // No link: say what the card sees, at 5, 10, 20 and 40 seconds,
            // then every 2 minutes
            static uint64_t started, next_report;
            if (!started) {
                started = now;
                next_report = now + 5000000000ull;
            }
            if (link_up != 1 && now >= next_report) {
                link_report();
                uint64_t since = now - started;
                next_report = now + (since < 40000000000ull ? since : 120000000000ull);
            }
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    rx_malloc_commit(1);
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "igc: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    device_id = (uint16_t)(rx_drv_cfg_read(dev, 0, 4) >> 16);
    // The 82576 family (QEMU's igb) by its IDs; everything else listed is an I225/I226
    i225 = !(device_id == 0x10C9 || device_id == 0x10E6 || device_id == 0x10E7 || device_id == 0x10E8 || device_id == 0x1526 ||
             device_id == 0x150A || device_id == 0x1518 || device_id == 0x150D);
    ICR = i225 ? 0x1500 : 0x00C0;
    IMS = i225 ? 0x1508 : 0x00D0;
    IMC = i225 ? 0x150C : 0x00D8;
    regs = rx_drv_map_bar(dev, 0, 0);
    if (!regs) return fail("cannot map BAR 0");
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    rx_drv_bus_master(dev, 1);
    if (reset() < 0) return fail("the card did not come out of reset");
    read_mac();
    if (rings() < 0) return fail("cannot allocate the rings");
    uint32_t used = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSI, 0, &irq, &used) < 0) polling = 1;
    wr(IMS, INT_LSC | INT_RXT0 | INT_RXDMT0 | INT_RXO | INT_TXDW);
    link_up = -1;
    printf("igc %s: %04x (%s), %02x:%02x:%02x:%02x:%02x:%02x%s\n", where, device_id, i225 ? "I225/I226" : "82576", mac[0], mac[1],
           mac[2], mac[3], mac[4], mac[5], polling ? " (polling)" : "");
    check_link();

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
        printf("igc %s: netd is gone; waiting for it\n", where);
    }
}
