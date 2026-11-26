// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// rtl8125: Realtek's 2.5 GbE RTL8125 (A, B, D; the Killer E3000) and 5 GbE
// RTL8126 (A), the wired card of many AM5 and Intel desktop boards; the
// desktop PC's is an RTL8126 (10ec:8126 rev 01, 2026-11-26, CHECKLIST.md
// "Fourth round"). Like igc and e1000e, it moves Ethernet frames between
// the card and netd over /svc/netdev (<rustux/netdev.h>) and nothing more.
// devmgr starts it for the IDs in /etc/drivers.
//
//   rtl8125 BDF
//
// The registers are in BAR 2 (BAR 0 is the same registers as I/O ports).
// Queue 0 only, with the classic 16-byte descriptors of the r8169 family
// (the 8125's newer format is turned off, as Linux does): a receive ring
// and a transmit ring of 256, 2 KiB buffers, no offloads (netd computes
// the checksums). The set-up follows Linux's r8169 (rtl_hw_start_8125 and
// rtl_hw_start_8125_common): the receive gate opened, RSS and interrupt
// moderation off, the MAC's OCP settings. Linux also loads a PHY patch
// (rtl_nic/rtl8126a-*.fw) when it has one and works without it; this
// driver does not load one. MSI wakes the receive thread, which also looks
// every millisecond; the link state from PHYstatus.
//
// QEMU emulates no card of this family, so this driver is tested on the
// hardware only.

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

#define MAC0          0x00
#define MAC4          0x04
#define MAR0          0x08
#define TX_DESC_LO    0x20
#define TX_DESC_HI    0x24
#define INT_CFG0_8125 0x34
#define CHIP_CMD      0x37
#define INTR_MASK     0x38   // 32 bits on the 8125 family
#define INTR_STATUS   0x3C   // 32 bits; 1s written clear
#define TX_CONFIG     0x40
#define RX_CONFIG     0x44
#define CFG9346       0x50
#define CONFIG1       0x52
#define CONFIG2       0x53
#define CONFIG5       0x56
#define PHY_STATUS    0x6C
#define PMCH          0x6F
#define INT_CFG1_8125 0x7A
#define TX_POLL_8125  0x90
#define MAC_OCP       0xB0
#define GPHY_OCP      0xB8
#define RX_MAX_SIZE   0xDA
#define CPLUS_CMD     0xE0
#define INTR_MITIGATE 0xE2
#define RX_DESC_LO    0xE4
#define RX_DESC_HI    0xE8
#define MISC          0xF0
#define RSS_CTRL_8125 0x4500
#define Q_NUM_CTRL_8125 0x4800
#define MAC0_BKP      0x19E0

#define CMD_RESET     0x10
#define CMD_RX_EN     0x08
#define CMD_TX_EN     0x04
#define CFG_UNLOCK    0xC0
#define CFG_LOCK      0x00
#define RXDV_GATED_EN (1u << 19)
#define OCP_FLAG      0x80000000u

#define ACCEPT_MY     0x02
#define ACCEPT_MULTI  0x04
#define ACCEPT_BCAST  0x08
#define RX_FETCH_DFLT_8125 (8u << 27)
#define RX_DMA_BURST  (7u << 8)
#define TX_DMA_BURST  (7u << 8)
#define TX_IFG        (3u << 24)
#define TX_AUTO_FIFO  (1u << 7)

#define INT_RX_OK     0x0001
#define INT_RX_ERR    0x0002
#define INT_TX_OK     0x0004
#define INT_TX_ERR    0x0008
#define INT_RX_OVF    0x0010
#define INT_LINK      0x0020
#define INT_FIFO_OVF  0x0040

#define PHY_FULL      0x0001
#define PHY_LINK      0x0002
#define PHY_10        0x0004
#define PHY_100       0x0008
#define PHY_1000      0x0010
#define PHY_2500      0x0400
#define PHY_5000      0x1000

#define DESC_OWN      (1u << 31)
#define DESC_END      (1u << 30)
#define DESC_FIRST    (1u << 29)
#define DESC_LAST     (1u << 28)
#define RX_RES        (1u << 21)

#define NRX   256
#define NTX   256
#define BUFSZ 2048

struct desc { uint32_t opts1, opts2; uint64_t addr; };

static const char *where = "?";
static rx_handle_t dev, irq, bti;
static volatile uint8_t *regs;
static int polling, is8126;
static uint16_t device_id;
static uint32_t xid;
static rx_dma_buf_t rx_ring, tx_ring, rx_bufs, tx_bufs;
static volatile struct desc *rxr, *txr;
static uint32_t rx_next, tx_next;
static uint8_t tx_busy[NTX];
static uint8_t mac[6];
static int link_up;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t netd;

static uint8_t rd8(uint32_t o) { return *(volatile uint8_t *)(regs + o); }
static uint16_t rd16(uint32_t o) { return *(volatile uint16_t *)(regs + o); }
static uint32_t rd32(uint32_t o) { return *(volatile uint32_t *)(regs + o); }
static void wr8(uint32_t o, uint8_t v) { *(volatile uint8_t *)(regs + o) = v; }
static void wr16(uint32_t o, uint16_t v) { *(volatile uint16_t *)(regs + o) = v; }
static void wr32(uint32_t o, uint32_t v) { *(volatile uint32_t *)(regs + o) = v; }

static void sleep_us(long us) {
    struct timespec ts = { us / 1000000, (us % 1000000) * 1000 };
    nanosleep(&ts, 0);
}

static int fail(const char *what) {
    fprintf(stderr, "rtl8125 %s: %s\n", where, what);
    return 1;
}

// ---- OCP: the MAC's internal registers and the PHY's ----

static uint16_t mac_ocp_rd(uint32_t reg) {
    wr32(MAC_OCP, reg << 15);
    return (uint16_t)rd32(MAC_OCP);
}

static void mac_ocp_wr(uint32_t reg, uint16_t v) { wr32(MAC_OCP, OCP_FLAG | reg << 15 | v); }

static void mac_ocp_mod(uint32_t reg, uint16_t clear, uint16_t set) {
    mac_ocp_wr(reg, (uint16_t)((mac_ocp_rd(reg) & ~clear) | set));
}

static int phy_ocp_wr(uint32_t reg, uint16_t v) {
    wr32(GPHY_OCP, OCP_FLAG | reg << 15 | v);
    for (int k = 0; k < 100; k++) {
        if (!(rd32(GPHY_OCP) & OCP_FLAG)) return 0;
        sleep_us(10);
    }
    return -1;
}

static int phy_ocp_rd(uint32_t reg, uint16_t *v) {
    wr32(GPHY_OCP, reg << 15);
    for (int k = 0; k < 100; k++) {
        uint32_t x = rd32(GPHY_OCP);
        if (x & OCP_FLAG) {
            *v = (uint16_t)x;
            return 0;
        }
        sleep_us(10);
    }
    return -1;
}

// The standard MII registers are at OCP 0xA400 + 2 * register
static int mii_rd(int reg, uint16_t *v) { return phy_ocp_rd(0xA400 + 2u * (uint32_t)reg, v); }
static int mii_wr(int reg, uint16_t v) { return phy_ocp_wr(0xA400 + 2u * (uint32_t)reg, v); }

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
    h.link = (uint16_t)(link_up > 0);
    snprintf(h.driver, sizeof h.driver, "rtl8125 %s", where);
    send_netd(RX_NETDEV_HELLO, 1500, &h, sizeof h);
}

// ---- the card ----

static int mac_ok(const uint8_t *m) {
    int zero = 1, ones = 1;
    for (int i = 0; i < 6; i++) {
        if (m[i]) zero = 0;
        if (m[i] != 0xFF) ones = 0;
    }
    return !zero && !ones && !(m[0] & 1);
}

static void read_mac(void) {
    for (int i = 0; i < 6; i++) mac[i] = rd8(MAC0 + (uint32_t)i);
    if (mac_ok(mac)) return;
    // The copy the chip keeps from its EEPROM (Linux reads it first)
    uint32_t lo = rd32(MAC0_BKP);
    uint16_t hi = rd16(MAC0_BKP + 4);
    uint8_t b[6] = { (uint8_t)lo, (uint8_t)(lo >> 8), (uint8_t)(lo >> 16), (uint8_t)(lo >> 24), (uint8_t)hi, (uint8_t)(hi >> 8) };
    if (mac_ok(b)) {
        memcpy(mac, b, 6);
    } else {
        // A locally administered address of our own
        uint64_t t = rx_clock_get_monotonic();
        uint8_t m[6] = { 0x02, 0x52, 0x58, (uint8_t)(t >> 16), (uint8_t)(t >> 8), (uint8_t)t };
        memcpy(mac, m, 6);
    }
    wr8(CFG9346, CFG_UNLOCK);
    wr32(MAC4, (uint32_t)mac[4] | (uint32_t)mac[5] << 8);
    rd32(MAC4);
    wr32(MAC0, (uint32_t)mac[0] | (uint32_t)mac[1] << 8 | (uint32_t)mac[2] << 16 | (uint32_t)mac[3] << 24);
    rd32(MAC0);
    wr8(CFG9346, CFG_LOCK);
}

// Stop whatever the firmware (its network boot) left running, then the
// chip's own reset
static int reset(void) {
    wr32(INTR_MASK, 0);
    wr32(INTR_STATUS, 0xFFFFFFFFu);
    wr32(MISC, rd32(MISC) | RXDV_GATED_EN);
    wr8(CHIP_CMD, (uint8_t)(rd8(CHIP_CMD) & ~(CMD_RX_EN | CMD_TX_EN)));
    for (int k = 0; k < 42 && (rd16(INTR_MITIGATE) & 0x0103) != 0x0103; k++) sleep_us(100);
    wr8(CHIP_CMD, CMD_RESET);
    for (int k = 0; k < 100 && (rd8(CHIP_CMD) & CMD_RESET); k++) sleep_us(100);
    if (rd8(CHIP_CMD) & CMD_RESET) return -1;
    // The PLL powered up
    wr8(PMCH, (uint8_t)(rd8(PMCH) | 0xC0));
    return 0;
}

// Linux's rtl_hw_start_8125_common, without the power saving (EEE, ASPM)
static void start_common(void) {
    wr16(0x382, 0x221B);
    wr32(RSS_CTRL_8125, 0);
    wr16(Q_NUM_CTRL_8125, 0);
    mac_ocp_mod(0xD40A, 0x0010, 0x0000);          // no UPS
    wr8(CONFIG1, (uint8_t)(rd8(CONFIG1) & ~0x10));
    mac_ocp_wr(0xC140, 0xFFFF);
    mac_ocp_wr(0xC142, 0xFFFF);
    mac_ocp_mod(0xD3E2, 0x0FFF, 0x03A9);
    mac_ocp_mod(0xD3E4, 0x00FF, 0x0000);
    mac_ocp_mod(0xE860, 0x0000, 0x0080);
    mac_ocp_mod(0xEB58, 0x0001, 0x0000);          // the classic descriptor format
    if (is8126) mac_ocp_mod(0xE614, 0x0F00, 0x0F00);
    else mac_ocp_mod(0xE614, 0x0700, 0x0400);
    mac_ocp_mod(0xE63E, 0x0C30, 0x0020);
    mac_ocp_mod(0xC0B4, 0x0000, 0x000C);
    mac_ocp_mod(0xEB6A, 0x00FF, 0x0033);
    mac_ocp_mod(0xEB50, 0x03E0, 0x0040);
    mac_ocp_mod(0xE056, 0x00F0, 0x0030);
    mac_ocp_mod(0xE040, 0x1000, 0x0000);
    mac_ocp_mod(0xEA1C, 0x0003, 0x0001);
    mac_ocp_mod(0xE0C0, 0x4F0F, 0x4403);
    mac_ocp_mod(0xE052, 0x0080, 0x0068);
    mac_ocp_mod(0xD430, 0x0FFF, 0x047F);
    mac_ocp_mod(0xEA1C, 0x0004, 0x0000);
    mac_ocp_mod(0xEB54, 0x0000, 0x0001);
    sleep_us(1);
    mac_ocp_mod(0xEB54, 0x0001, 0x0000);
    wr16(0x1880, (uint16_t)(rd16(0x1880) & ~0x0030));
    mac_ocp_wr(0xE098, 0xC302);
    for (int k = 0; k < 1000 && (mac_ocp_rd(0xE00E) & 0x2000); k++) sleep_us(10);
    // The receive gate open
    wr32(MISC, rd32(MISC) & ~RXDV_GATED_EN);
}

// The PHY powered up and auto-negotiating everything it can, 2.5 and 5 Gb/s too
static void phy_up(void) {
    uint16_t bmcr = 0, id1 = 0, id2 = 0, adv = 0;
    if (mii_rd(2, &id1) < 0 || mii_rd(3, &id2) < 0 || mii_rd(0, &bmcr) < 0) {
        printf("rtl8125 %s: the PHY does not answer; leaving it as it is\n", where);
        return;
    }
    // 2500 (bit 7) and 5000 (bit 8) full duplex in the PHY's 0xA5D4
    if (phy_ocp_rd(0xA5D4, &adv) == 0) phy_ocp_wr(0xA5D4, (uint16_t)(adv | 0x0080 | (is8126 ? 0x0100 : 0)));
    uint16_t an = 0, gt = 0;
    if (mii_rd(4, &an) == 0) mii_wr(4, (uint16_t)(an | 0x01E0 | 0x0001));
    if (mii_rd(9, &gt) == 0) mii_wr(9, (uint16_t)(gt | 0x0200));
    uint16_t nb = (uint16_t)((bmcr & ~(1u << 11)) | (1u << 12) | (1u << 9)); // not powered down, auto-neg, restart
    mii_wr(0, nb);
    printf("rtl8125 %s: PHY %04x:%04x, control %04x -> %04x, multi-gig %04x\n", where, id1, id2, bmcr, nb, adv);
}

static void rx_give(uint32_t i) {
    rxr[i].addr = rx_bufs.pa + (uint64_t)i * BUFSZ;
    rxr[i].opts2 = 0;
    rx_dma_barrier();
    rxr[i].opts1 = DESC_OWN | (i == NRX - 1 ? DESC_END : 0) | BUFSZ;
}

static int start(void) {
    if (rx_dma_alloc(bti, NRX * sizeof(struct desc), 0, &rx_ring) < 0 || rx_dma_alloc(bti, NTX * sizeof(struct desc), 0, &tx_ring) < 0 ||
        rx_dma_alloc(bti, (uint64_t)NRX * BUFSZ, 0, &rx_bufs) < 0 || rx_dma_alloc(bti, (uint64_t)NTX * BUFSZ, 0, &tx_bufs) < 0)
        return -1;
    rxr = (volatile struct desc *)rx_ring.va;
    txr = (volatile struct desc *)tx_ring.va;
    memset((void *)txr, 0, NTX * sizeof(struct desc));
    txr[NTX - 1].opts1 = DESC_END;
    for (uint32_t i = 0; i < NRX; i++) rx_give(i);
    rx_next = tx_next = 0;
    memset(tx_busy, 0, sizeof tx_busy);

    wr8(CFG9346, CFG_UNLOCK);
    // No ASPM or clock requests (Linux turns them off while running)
    wr8(CONFIG2, (uint8_t)(rd8(CONFIG2) & ~0x80));
    wr8(CONFIG5, (uint8_t)(rd8(CONFIG5) & ~0x01));
    wr16(CPLUS_CMD, (uint16_t)(rd16(CPLUS_CMD) & ~0x0060)); // no checksum or VLAN offload on receive
    // No interrupt moderation
    for (uint32_t r = 0xA00; r < (is8126 ? 0xA80u : 0xB00u); r += 4) wr32(r, 0);
    wr8(INT_CFG0_8125, 0);
    wr16(INT_CFG1_8125, 0);
    start_common();
    wr16(RX_MAX_SIZE, BUFSZ);
    wr32(TX_DESC_HI, (uint32_t)(tx_ring.pa >> 32));
    wr32(TX_DESC_LO, (uint32_t)tx_ring.pa);
    wr32(RX_DESC_HI, (uint32_t)(rx_ring.pa >> 32));
    wr32(RX_DESC_LO, (uint32_t)rx_ring.pa);
    wr8(CFG9346, CFG_LOCK);
    rd8(CHIP_CMD);
    wr8(CHIP_CMD, CMD_TX_EN | CMD_RX_EN);
    wr32(RX_CONFIG, RX_FETCH_DFLT_8125 | RX_DMA_BURST | ACCEPT_MY | ACCEPT_MULTI | ACCEPT_BCAST);
    wr32(TX_CONFIG, TX_DMA_BURST | TX_IFG | TX_AUTO_FIFO);
    // Every multicast address: IPv6's neighbour discovery needs them
    wr32(MAR0, 0xFFFFFFFFu);
    wr32(MAR0 + 4, 0xFFFFFFFFu);
    return 0;
}

// Frames the card received, to netd; descriptors back to the card
static void rx_drain(void) {
    for (int n = 0; n < NRX; n++) {
        uint32_t o = rxr[rx_next].opts1;
        if (o & DESC_OWN) break;
        rx_dma_barrier();
        uint32_t len = o & 0x3FFF;
        // A whole frame in one buffer, without an error; the length counts the FCS
        if ((o & DESC_FIRST) && (o & DESC_LAST) && !(o & RX_RES) && len > 4 && len - 4 <= RX_NETDEV_MAX_FRAME)
            send_netd(RX_NETDEV_FRAME, 0, (const void *)(rx_bufs.va + (uint64_t)rx_next * BUFSZ), len - 4);
        rx_give(rx_next);
        rx_next = (rx_next + 1) % NRX;
    }
}

static void transmit(const uint8_t *frame, uint32_t len) {
    if (len > BUFSZ || len < 14) return;
    pthread_mutex_lock(&lock);
    volatile struct desc *d = &txr[tx_next];
    if (tx_busy[tx_next] && (d->opts1 & DESC_OWN)) {
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
    d->opts2 = 0;
    rx_dma_barrier();
    d->opts1 = DESC_OWN | DESC_FIRST | DESC_LAST | (tx_next == NTX - 1 ? DESC_END : 0) | len;
    tx_busy[tx_next] = 1;
    tx_next = (tx_next + 1) % NTX;
    rx_dma_barrier();
    wr16(TX_POLL_8125, 1);
    pthread_mutex_unlock(&lock);
}

static void check_link(void) {
    uint16_t st = rd16(PHY_STATUS);
    int up = (st & PHY_LINK) != 0;
    if (up != link_up) {
        link_up = up;
        const char *speed = st & PHY_5000 ? "5000" : st & PHY_2500 ? "2500" : st & PHY_1000 ? "1000" : st & PHY_100 ? "100" : "10";
        if (up) printf("rtl8125 %s: link up, %s Mb/s %s duplex\n", where, speed, (st & PHY_FULL) ? "full" : "half");
        else printf("rtl8125 %s: link down\n", where);
        send_netd(RX_NETDEV_LINK, (uint32_t)up, 0, 0);
    }
}

// What the card and its PHY say, for the log while there is no link
static void link_report(void) {
    uint16_t bmcr = 0, bmsr = 0, lpa = 0;
    mii_rd(0, &bmcr);
    mii_rd(1, &bmsr);
    mii_rd(1, &bmsr); // latched: the second read is now
    mii_rd(5, &lpa);
    printf("rtl8125 %s: no link yet: PHYstatus %04x ChipCmd %02x MISC %08x; BMCR %04x BMSR %04x partner %04x\n", where,
           rd16(PHY_STATUS), rd8(CHIP_CMD), rd32(MISC), bmcr, bmsr, lpa);
}

static void *irq_thread(void *arg) {
    (void)arg;
    uint64_t last = 0, started = 0, next_report = 0;
    for (;;) {
        if (polling) sleep_us(1000);
        else rx_drv_wait_irq(irq, 1);
        uint32_t cause = rd32(INTR_STATUS);
        if (cause) wr32(INTR_STATUS, cause);
        pthread_mutex_lock(&lock);
        rx_drain();
        pthread_mutex_unlock(&lock);
        if (cause & (INT_RX_OVF | INT_FIFO_OVF)) printf("rtl8125 %s: receive overflow (%08x)\n", where, cause);
        uint64_t now = rx_clock_get_monotonic();
        if ((cause & INT_LINK) || now - last > 1000000000ull) {
            last = now;
            check_link();
            // No link: say what the card sees, at 5, 10, 20 and 40 seconds, then every 2 minutes
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

static const char *chip_name(void) {
    switch (xid) {
    case 0x609: return "RTL8125A";
    case 0x641: return "RTL8125B";
    case 0x688: case 0x689: return "RTL8125D";
    case 0x649: case 0x64A: return "RTL8126A";
    default: return is8126 ? "RTL8126 (unknown revision)" : "RTL8125 (unknown revision)";
    }
}

int main(int argc, char **argv) {
    rx_malloc_commit(1);
    if (argc > 1) where = argv[1];
    dev = rx_drv_device();
    if (!dev) {
        fprintf(stderr, "rtl8125: started without a device (devmgr starts this driver)\n");
        return 2;
    }
    device_id = (uint16_t)(rx_drv_cfg_read(dev, 0, 4) >> 16);
    is8126 = device_id == 0x8126 || device_id == 0x5000;
    // The registers: BAR 2 (memory); BAR 0 is the same as I/O ports
    regs = rx_drv_map_bar(dev, 2, 0);
    if (!regs) regs = rx_drv_map_bar(dev, 1, 0);
    if (!regs) return fail("cannot map the registers (BAR 2)");
    if (rx_drv_bti(dev, &bti) < 0) return fail("no BTI");
    rx_drv_bus_master(dev, 1);
    xid = (rd32(TX_CONFIG) >> 20) & 0xFCF;
    if (xid == 0xFCF) return fail("the registers read as all ones (the card is not answering)");
    if (reset() < 0) return fail("the card did not come out of reset");
    read_mac();
    if (start() < 0) return fail("cannot allocate the rings");
    phy_up();
    uint32_t used = 0;
    if (rx_drv_interrupt(dev, RX_DEV_IRQ_MSI, 0, &irq, &used) < 0) polling = 1;
    wr32(INTR_MASK, INT_RX_OK | INT_RX_ERR | INT_TX_OK | INT_TX_ERR | INT_RX_OVF | INT_LINK | INT_FIFO_OVF);
    link_up = -1;
    printf("rtl8125 %s: %04x %s (XID %03x), %02x:%02x:%02x:%02x:%02x:%02x%s\n", where, device_id, chip_name(), xid, mac[0], mac[1], mac[2],
           mac[3], mac[4], mac[5], polling ? " (polling)" : "");
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
        printf("rtl8125 %s: netd is gone; waiting for it\n", where);
    }
}
