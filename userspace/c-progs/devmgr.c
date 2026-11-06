// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// devmgr: the PCI bus driver and device manager (step 12,
// docs/DRIVERS.md, protocols in <rustux/device.h>).
//
// - Reads PCI configuration space through ECAM (the ACPI MCFG table, one
//   uncached physical VMO mapped a bus at a time) or, without MCFG,
//   through ports 0xCF8/0xCFC; walks bus 0 and every bus behind a bridge.
// - Records each function: IDs, class, BARs (sized, except for devices
//   the kernel itself uses: display and USB controllers), MSI/MSI-X.
// - Starts a driver for each device that /etc/drivers names, giving it
//   one channel (the device channel) through which it gets only its own
//   device's resources; restarts a driver that ends (up to 5 quick
//   failures in a row).
// - Serves /svc/devmgr: the device list (lspci), block devices (publish,
//   list for lsblk, connect).
//
// init starts it with the root resource; nobody else holds that.

#include <errno.h>
#include <rustux/block.h>
#include <rustux/device.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_PCI     256
#define MAX_CLIENTS 64
#define MAX_BLOCK   32

static rx_handle_t resource;

// ---- configuration space ----

static int use_ecam;
static uint64_t ecam_phys;
static uint8_t ecam_start, ecam_end;
static rx_handle_t ecam_vmo;
static volatile uint8_t *ecam_bus[256]; // mapped 1 MiB windows

struct pci {
    rx_pci_info_t info;
    uint32_t bar_raw[6];
    char prog[128];         // driver from /etc/drivers ("" if none)
    rx_handle_t chan;       // our end of the device channel
    rx_handle_t proc;       // the driver process
    uint64_t started;       // when it was (re)started, ns
    int quick_fails;
    int given_up;
};

static struct pci devs[MAX_PCI];
static int ndevs;

static volatile uint8_t *ecam_addr(const rx_pci_info_t *d, uint16_t off) {
    if (d->bus < ecam_start || d->bus > ecam_end) return 0;
    volatile uint8_t *b = ecam_bus[d->bus];
    if (!b) {
        uint64_t addr = 0;
        if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, ecam_vmo,
                        (uint64_t)(d->bus - ecam_start) << 20, 1 << 20, &addr) != RX_OK) return 0;
        b = ecam_bus[d->bus] = (volatile uint8_t *)addr;
    }
    return b + ((uint32_t)d->dev << 15 | (uint32_t)d->func << 12 | off);
}

static uint32_t cf8(const rx_pci_info_t *d, uint16_t off) {
    return 0x80000000u | (uint32_t)d->bus << 16 | (uint32_t)d->dev << 11 | (uint32_t)d->func << 8 | (off & 0xFC);
}

static uint32_t cfg_read(const rx_pci_info_t *d, uint16_t off, int width) {
    if (use_ecam) {
        volatile uint8_t *p = ecam_addr(d, off);
        if (!p) return 0xFFFFFFFF;
        if (width == 1) return *p;
        if (width == 2) return *(volatile uint16_t *)p;
        return *(volatile uint32_t *)p;
    }
    if (off >= 256) return 0xFFFFFFFF;
    rx_outl(0xCF8, cf8(d, off));
    if (width == 1) return rx_inb(0xCFC + (off & 3));
    if (width == 2) return rx_inw(0xCFC + (off & 2));
    return rx_inl(0xCFC);
}

static void cfg_write(const rx_pci_info_t *d, uint16_t off, int width, uint32_t v) {
    if (use_ecam) {
        volatile uint8_t *p = ecam_addr(d, off);
        if (!p) return;
        if (width == 1) *p = (uint8_t)v;
        else if (width == 2) *(volatile uint16_t *)p = (uint16_t)v;
        else *(volatile uint32_t *)p = v;
        return;
    }
    if (off >= 256) return;
    rx_outl(0xCF8, cf8(d, off));
    if (width == 1) rx_outb(0xCFC + (off & 3), (uint8_t)v);
    else if (width == 2) rx_outw(0xCFC + (off & 2), (uint16_t)v);
    else rx_outl(0xCFC, v);
}

#define R8(d, o)  ((uint8_t)cfg_read((d), (o), 1))
#define R16(d, o) ((uint16_t)cfg_read((d), (o), 2))
#define R32(d, o) cfg_read((d), (o), 4)

static int setup_ecam(void) {
    static unsigned char mcfg[4096];
    int64_t n = rx_acpi_table_read(resource, "MCFG", 0, mcfg, sizeof mcfg);
    if (n < 44 + 16) return 0;
    // Header (36) + 8 reserved, then 16-byte entries; use segment 0
    for (int64_t off = 44; off + 16 <= n && off + 16 <= (int64_t)sizeof mcfg; off += 16) {
        uint64_t base;
        uint16_t seg;
        memcpy(&base, mcfg + off, 8);
        memcpy(&seg, mcfg + off + 8, 2);
        if (seg != 0) continue;
        ecam_start = mcfg[off + 10];
        ecam_end = mcfg[off + 11];
        ecam_phys = base + ((uint64_t)ecam_start << 20);
        uint64_t size = ((uint64_t)ecam_end - ecam_start + 1) << 20;
        if (rx_vmo_create_physical(resource, ecam_phys, size, &ecam_vmo) != RX_OK) {
            printf("devmgr: cannot map ECAM at %#llx\n", (unsigned long long)ecam_phys);
            return 0;
        }
        return 1;
    }
    return 0;
}

// ---- enumeration ----

// The kernel drives the USB keyboard (xHCI) and draws on the screen
// (display controllers): leave those devices alone
static int kernel_owned(const rx_pci_info_t *i) {
    return i->class_code == 0x03 || (i->class_code == 0x0C && i->subclass == 0x03);
}

static void size_bars(struct pci *p) {
    rx_pci_info_t *i = &p->info;
    int nbars = (i->header_type & 0x7F) == 0 ? 6 : (i->header_type & 0x7F) == 1 ? 2 : 0;
    int can_size = !(i->flags & RX_PCI_FLAG_KERNEL) && i->class_code != 0x06;
    uint16_t cmd = R16(i, 4);
    if (can_size) cfg_write(i, 4, 2, cmd & ~3u); // decoding off while sizing
    for (int b = 0; b < nbars; b++) {
        uint16_t off = (uint16_t)(0x10 + 4 * b);
        uint32_t raw = R32(i, off);
        p->bar_raw[b] = raw;
        rx_pci_bar_t *bar = &i->bars[b];
        if (raw & 1) {
            bar->type = RX_PCI_BAR_IO;
            bar->address = raw & ~3u;
            if (can_size) {
                cfg_write(i, off, 4, 0xFFFFFFFF);
                uint32_t m = R32(i, off) & ~3u;
                cfg_write(i, off, 4, raw);
                if (!(m & 0xFFFF0000u)) m |= 0xFFFF0000u;
                bar->size = m ? (uint32_t)(~m + 1) : 0;
            }
        } else {
            int t = (raw >> 1) & 3;
            bar->type = RX_PCI_BAR_MEM;
            bar->address = raw & ~0xFu;
            if (raw & 8) bar->flags |= RX_PCI_BAR_PREFETCH;
            uint32_t hi = 0;
            if (t == 2 && b + 1 < nbars) {
                bar->flags |= RX_PCI_BAR_64BIT;
                hi = R32(i, off + 4);
                bar->address |= (uint64_t)hi << 32;
            }
            if (can_size) {
                cfg_write(i, off, 4, 0xFFFFFFFF);
                uint64_t m = R32(i, off) & ~0xFu;
                cfg_write(i, off, 4, raw);
                if (bar->flags & RX_PCI_BAR_64BIT) {
                    cfg_write(i, off + 4, 4, 0xFFFFFFFF);
                    m |= (uint64_t)R32(i, off + 4) << 32;
                    cfg_write(i, off + 4, 4, hi);
                } else if (m) {
                    m |= 0xFFFFFFFF00000000ull;
                }
                // No writable address bits: the BAR is not implemented
                bar->size = m ? ~m + 1 : 0;
            }
            if (bar->flags & RX_PCI_BAR_64BIT) {
                b++; // the upper half
                p->bar_raw[b] = hi;
            }
        }
        if (bar->address == 0 && bar->size == 0) bar->type = RX_PCI_BAR_NONE;
    }
    if (can_size) cfg_write(i, 4, 2, cmd);
}

static void read_caps(rx_pci_info_t *i) {
    if (!(R16(i, 6) & 0x10)) return;
    uint8_t ptr = R8(i, (i->header_type & 0x7F) == 2 ? 0x14 : 0x34) & ~3u;
    for (int guard = 0; ptr >= 0x40 && guard < 48; guard++) {
        uint8_t id = R8(i, ptr);
        if (id == RX_PCI_CAP_MSI && !i->msi) {
            i->msi = ptr;
            i->msi_count = (uint16_t)(1u << ((R16(i, ptr + 2) >> 1) & 7));
        } else if (id == RX_PCI_CAP_MSIX && !i->msix) {
            i->msix = ptr;
            i->msix_count = (uint16_t)((R16(i, ptr + 2) & 0x7FF) + 1);
        } else if (id == RX_PCI_CAP_PCIE && !i->pcie) {
            i->pcie = ptr;
        }
        ptr = R8(i, ptr + 1) & ~3u;
    }
}

static void scan_bus(uint8_t bus, int depth);

static void add_function(uint8_t bus, uint8_t dev, uint8_t fn, int depth) {
    if (ndevs >= MAX_PCI) return;
    struct pci *p = &devs[ndevs];
    memset(p, 0, sizeof *p);
    rx_pci_info_t *i = &p->info;
    i->bus = bus;
    i->dev = dev;
    i->func = fn;
    uint32_t id = R32(i, 0);
    if ((id & 0xFFFF) == 0xFFFF || id == 0) return;
    ndevs++;
    i->vendor = (uint16_t)id;
    i->device = (uint16_t)(id >> 16);
    uint32_t cls = R32(i, 8);
    i->revision = (uint8_t)cls;
    i->prog_if = (uint8_t)(cls >> 8);
    i->subclass = (uint8_t)(cls >> 16);
    i->class_code = (uint8_t)(cls >> 24);
    i->header_type = R8(i, 0x0E);
    if ((i->header_type & 0x7F) == 0) {
        i->subsystem_vendor = R16(i, 0x2C);
        i->subsystem = R16(i, 0x2E);
    }
    i->irq_line = R8(i, 0x3C);
    i->irq_pin = R8(i, 0x3D);
    if (use_ecam) i->flags |= RX_PCI_FLAG_ECAM;
    if (kernel_owned(i)) i->flags |= RX_PCI_FLAG_KERNEL;
    read_caps(i);
    size_bars(p);
    i->command = R16(i, 4);
    i->status = R16(i, 6);
    // A PCI-to-PCI bridge: scan the bus behind it
    if ((i->header_type & 0x7F) == 1 && depth < 16) {
        uint8_t secondary = R8(i, 0x19);
        if (secondary > bus) scan_bus(secondary, depth + 1);
    }
}

static void scan_bus(uint8_t bus, int depth) {
    for (uint8_t dev = 0; dev < 32; dev++) {
        rx_pci_info_t probe = { .bus = bus, .dev = dev, .func = 0 };
        uint32_t id = R32(&probe, 0);
        if ((id & 0xFFFF) == 0xFFFF || id == 0) continue;
        int multi = R8(&probe, 0x0E) & 0x80;
        for (uint8_t fn = 0; fn < (multi ? 8 : 1); fn++) add_function(bus, dev, fn, depth);
    }
}

// ---- /etc/drivers ----

// Up to `max` hex numbers separated by ':' (returns how many)
static int hex_list(const char *s, unsigned long *v, int max) {
    int n = 0;
    while (n < max) {
        char *end;
        v[n] = strtoul(s, &end, 16);
        if (end == s) break;
        n++;
        if (*end != ':') break;
        s = end + 1;
    }
    return n;
}

static int matches(const char *pat, const rx_pci_info_t *i) {
    unsigned long v[3];
    if (!strncmp(pat, "class=", 6)) {
        int n = hex_list(pat + 6, v, 3);
        if (n < 1 || v[0] != i->class_code) return 0;
        if (n >= 2 && v[1] != i->subclass) return 0;
        if (n >= 3 && v[2] != i->prog_if) return 0;
        return 1;
    }
    return hex_list(pat, v, 2) == 2 && v[0] == i->vendor && v[1] == i->device;
}

// The next blank-separated word of *s into out (cap bytes); 0 if none
static int word(char **s, char *out, size_t cap) {
    char *p = *s;
    while (*p == ' ' || *p == '\t') p++;
    size_t n = 0;
    while (*p && *p != ' ' && *p != '\t' && *p != '\n' && *p != '\r') {
        if (n + 1 < cap) out[n++] = *p;
        p++;
    }
    out[n] = 0;
    *s = p;
    return n > 0;
}

static void read_drivers(void) {
    FILE *f = fopen("/etc/drivers", "r");
    if (!f) return;
    char line[256];
    while (fgets(line, sizeof line, f)) {
        char pat[64], prog[128], *s = line;
        if (line[0] == '#' || !word(&s, pat, sizeof pat) || !word(&s, prog, sizeof prog)) continue;
        for (int k = 0; k < ndevs; k++) {
            struct pci *p = &devs[k];
            if (p->prog[0] || (p->info.flags & RX_PCI_FLAG_KERNEL) || !matches(pat, &p->info)) continue;
            snprintf(p->prog, sizeof p->prog, "%s", prog);
            const char *base = strrchr(prog, '/');
            snprintf(p->info.driver, sizeof p->info.driver, "%s", base ? base + 1 : prog);
        }
    }
    fclose(f);
}

// ---- drivers ----

static uint64_t now_ns(void) { return rx_clock_get_monotonic(); }

static void bdf_name(const rx_pci_info_t *i, char *buf, size_t n) {
    snprintf(buf, n, "%02x:%02x.%x", i->bus, i->dev, i->func);
}

static void start_driver(struct pci *p) {
    rx_handle_t ours, theirs;
    if (rx_channel_create(0, &ours, &theirs) != RX_OK) return;
    char bdf[16];
    bdf_name(&p->info, bdf, sizeof bdf);
    char *argv[] = { p->info.driver, bdf, 0 };
    rx_spawn_handle_t hs[] = { { RX_PA_USER0, theirs } };
    rx_handle_t proc;
    if (rx_spawn(p->prog, argv, 0, RX_SPAWN_CLONE_ALL, hs, 1, &proc) != RX_OK) {
        printf("devmgr: cannot start %s for %s\n", p->prog, bdf);
        rx_handle_close(ours);
        p->given_up = 1;
        return;
    }
    p->chan = ours;
    p->proc = proc;
    p->started = now_ns();
    rx_info_handle_basic_t hb;
    if (rx_object_get_info(proc, RX_INFO_HANDLE_BASIC, &hb, sizeof hb, 0, 0) == RX_OK) p->info.driver_koid = hb.koid;
}

// Put the device back to rest after its driver is gone: no MSI, no DMA
static void quiesce(struct pci *p) {
    rx_pci_info_t *i = &p->info;
    if (i->msi) cfg_write(i, i->msi + 2, 2, R16(i, i->msi + 2) & ~1u);
    if (i->msix) cfg_write(i, i->msix + 2, 2, R16(i, i->msix + 2) & ~0x8000u);
    cfg_write(i, 4, 2, R16(i, 4) & ~4u);
}

static void driver_ended(struct pci *p) {
    int64_t code = rx_wait_process(p->proc);
    p->proc = 0;
    p->info.driver_koid = 0;
    if (p->chan) rx_handle_close(p->chan);
    p->chan = 0;
    quiesce(p);
    char bdf[16];
    bdf_name(&p->info, bdf, sizeof bdf);
    if (now_ns() - p->started < 5000000000ull) p->quick_fails++;
    else p->quick_fails = 0;
    if (p->quick_fails >= 5) {
        printf("devmgr: %s for %s keeps failing (code %lld); not restarting it\n", p->info.driver, bdf, (long long)code);
        p->given_up = 1;
        return;
    }
    printf("devmgr: %s for %s ended (code %lld); restarting it\n", p->info.driver, bdf, (long long)code);
    p->info.restarts++;
    start_driver(p);
}

// ---- the device channel ----

static char req[65536], rep[65536];

static int protected_reg(const rx_pci_info_t *i, uint16_t off, int width) {
    uint16_t end = (uint16_t)(off + width);
    if (off < 0x28 && end > 0x10) return 1;          // BARs
    if (off < 0x34 && end > 0x30) return 1;          // expansion ROM
    if (i->msi && off < i->msi + 24 && end > i->msi) return 1;
    if (i->msix && off < i->msix + 12 && end > i->msix) return 1;
    return 0;
}

static rx_handle_t get_bar(struct pci *p, uint32_t index, rx_pci_bar_t *out, int *err) {
    rx_pci_info_t *i = &p->info;
    *err = 0;
    if (index >= 6 || i->bars[index].type == RX_PCI_BAR_NONE) { *err = -ENOENT; return 0; }
    rx_pci_bar_t *b = &i->bars[index];
    *out = *b;
    rx_handle_t h = 0;
    if (b->type == RX_PCI_BAR_MEM) {
        if (!b->size) { *err = -ENOTSUP; return 0; }
        uint64_t base = b->address & ~0xFFFull;
        uint64_t len = (b->address + b->size - base + 0xFFF) & ~0xFFFull;
        rx_status_t s = rx_vmo_create_physical(resource, base, len, &h);
        if (s != RX_OK) { *err = -rx_status_to_errno(s); return 0; }
        cfg_write(i, 4, 2, R16(i, 4) | 2);
    } else {
        if (!b->size) { *err = -ENOTSUP; return 0; }
        rx_status_t s = rx_resource_create_range(resource, RX_RSRC_KIND_IOPORT, b->address, b->size, &h);
        if (s != RX_OK) { *err = -rx_status_to_errno(s); return 0; }
        cfg_write(i, 4, 2, R16(i, 4) | 1);
    }
    return h;
}

static int setup_msi(rx_pci_info_t *i, const rx_info_msi_t *m) {
    uint16_t c = R16(i, i->msi + 2);
    int is64 = c & 0x80, maskable = c & 0x100;
    cfg_write(i, i->msi + 4, 4, (uint32_t)m->address);
    if (is64) {
        cfg_write(i, i->msi + 8, 4, (uint32_t)(m->address >> 32));
        cfg_write(i, i->msi + 12, 2, m->data);
    } else {
        cfg_write(i, i->msi + 8, 2, m->data);
    }
    if (maskable) cfg_write(i, i->msi + (is64 ? 16 : 12), 4, 0);
    c = (uint16_t)((c & ~0x70u) | 1); // one vector, enabled
    cfg_write(i, i->msi + 2, 2, c);
    return 0;
}

static int setup_msix(struct pci *p, uint32_t index, const rx_info_msi_t *m) {
    rx_pci_info_t *i = &p->info;
    if (index >= i->msix_count) return -EINVAL;
    uint32_t t = R32(i, i->msix + 4);
    uint32_t bir = t & 7, toff = t & ~7u;
    if (bir >= 6 || i->bars[bir].type != RX_PCI_BAR_MEM || !i->bars[bir].size) return -EIO;
    uint64_t entry = i->bars[bir].address + toff + 16ull * index;
    uint64_t page = entry & ~0xFFFull;
    rx_handle_t vmo;
    if (rx_vmo_create_physical(resource, page, 4096, &vmo) != RX_OK) return -EIO;
    uint64_t addr = 0;
    rx_status_t s = rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, 4096, &addr);
    rx_handle_close(vmo);
    if (s != RX_OK) return -EIO;
    cfg_write(i, 4, 2, R16(i, 4) | 2);
    uint16_t c = R16(i, i->msix + 2);
    cfg_write(i, i->msix + 2, 2, c | 0x8000 | 0x4000); // enable, all masked
    volatile uint32_t *e = (volatile uint32_t *)(addr + (entry & 0xFFF));
    e[0] = (uint32_t)m->address;
    e[1] = (uint32_t)(m->address >> 32);
    e[2] = m->data;
    e[3] = e[3] & ~1u; // unmask this entry
    cfg_write(i, i->msix + 2, 2, (R16(i, i->msix + 2) | 0x8000) & ~0x4000u);
    rx_vmar_unmap(rx_vmar_root, addr, 4096);
    return 0;
}

static rx_handle_t get_interrupt(struct pci *p, uint32_t mode, uint32_t index, uint32_t *used, int *err) {
    rx_pci_info_t *i = &p->info;
    *err = 0;
    if (mode == RX_DEV_IRQ_ANY) mode = i->msix ? RX_DEV_IRQ_MSIX : RX_DEV_IRQ_MSI;
    if ((mode == RX_DEV_IRQ_MSI && (!i->msi || index != 0)) || (mode == RX_DEV_IRQ_MSIX && !i->msix) ||
        (mode != RX_DEV_IRQ_MSI && mode != RX_DEV_IRQ_MSIX)) {
        *err = -ENOTSUP; // INTx needs routing from ACPI (_PRT), not done yet
        return 0;
    }
    rx_handle_t irq;
    rx_status_t s = rx_interrupt_create(resource, 0, RX_INTERRUPT_MSI, &irq);
    if (s != RX_OK) { *err = -rx_status_to_errno(s); return 0; }
    rx_info_msi_t m;
    if (rx_object_get_info(irq, RX_INFO_INTERRUPT_MSI, &m, sizeof m, 0, 0) != RX_OK) {
        rx_handle_close(irq);
        *err = -EIO;
        return 0;
    }
    int r = mode == RX_DEV_IRQ_MSI ? setup_msi(i, &m) : setup_msix(p, index, &m);
    if (r < 0) {
        rx_handle_close(irq);
        *err = r;
        return 0;
    }
    // Messages need bus mastering; INTx off
    cfg_write(i, 4, 2, R16(i, 4) | 4 | 0x400);
    *used = mode;
    return irq;
}

static void reply(rx_handle_t ch, uint32_t txid, int status, const void *data, size_t len, rx_handle_t h) {
    rx_dev_reply_t r = { txid, status };
    memcpy(rep, &r, sizeof r);
    if (status < 0) len = 0;
    if (len) memcpy(rep + sizeof r, data, len);
    if (rx_channel_write(ch, 0, rep, (uint32_t)(sizeof r + len), h ? &h : 0, h ? 1 : 0) != RX_OK && h)
        rx_handle_close(h);
}

static void device_request(struct pci *p) {
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_status_t s = rx_channel_read(p->chan, 0, req, &h, sizeof req, 1, &nb, &nh);
    if (s != RX_OK) return; // PEER_CLOSED: the process-ended path cleans up
    if (nh) rx_handle_close(h);
    if (nb < sizeof(rx_dev_hdr_t)) return;
    rx_dev_hdr_t hdr;
    memcpy(&hdr, req, sizeof hdr);
    const char *body = req + sizeof hdr;
    size_t blen = nb - sizeof hdr;
    rx_pci_info_t *i = &p->info;
    switch (hdr.op) {
    case RX_DEV_OP_GET_INFO:
        i->command = R16(i, 4);
        i->status = R16(i, 6);
        reply(p->chan, hdr.txid, 0, i, sizeof *i, 0);
        break;
    case RX_DEV_OP_CONFIG_READ:
    case RX_DEV_OP_CONFIG_WRITE: {
        uint16_t f[2];
        uint32_t v = 0;
        if (blen < 4 + (hdr.op == RX_DEV_OP_CONFIG_WRITE ? 4u : 0u)) { reply(p->chan, hdr.txid, -EINVAL, 0, 0, 0); break; }
        memcpy(f, body, 4);
        int w = f[1];
        if ((w != 1 && w != 2 && w != 4) || f[0] % w || f[0] + w > (use_ecam ? 4096 : 256)) {
            reply(p->chan, hdr.txid, -EINVAL, 0, 0, 0);
            break;
        }
        if (hdr.op == RX_DEV_OP_CONFIG_READ) {
            v = cfg_read(i, f[0], w);
            reply(p->chan, hdr.txid, 0, &v, 4, 0);
        } else if (protected_reg(i, f[0], w)) {
            reply(p->chan, hdr.txid, -EPERM, 0, 0, 0);
        } else {
            memcpy(&v, body + 4, 4);
            cfg_write(i, f[0], w, v);
            reply(p->chan, hdr.txid, 0, 0, 0, 0);
        }
        break;
    }
    case RX_DEV_OP_GET_BAR: {
        uint32_t idx = 0;
        if (blen >= 4) memcpy(&idx, body, 4);
        rx_pci_bar_t b;
        int err;
        rx_handle_t bh = get_bar(p, idx, &b, &err);
        reply(p->chan, hdr.txid, err, &b, sizeof b, bh);
        break;
    }
    case RX_DEV_OP_GET_INTERRUPT: {
        uint32_t a[2] = { 0, 0 }, used = 0;
        if (blen >= 8) memcpy(a, body, 8);
        int err;
        rx_handle_t ih = get_interrupt(p, a[0], a[1], &used, &err);
        reply(p->chan, hdr.txid, err, &used, 4, ih);
        break;
    }
    case RX_DEV_OP_GET_BTI: {
        rx_handle_t bti;
        uint64_t id = (uint64_t)i->bus << 8 | (uint64_t)i->dev << 3 | i->func;
        rx_status_t st = rx_bti_create(resource, 0, id, &bti);
        reply(p->chan, hdr.txid, st == RX_OK ? 0 : -rx_status_to_errno(st), 0, 0, st == RX_OK ? bti : 0);
        break;
    }
    case RX_DEV_OP_BUS_MASTER: {
        uint32_t en = 0;
        if (blen >= 4) memcpy(&en, body, 4);
        uint16_t c = R16(i, 4);
        cfg_write(i, 4, 2, en ? (c | 4) : (c & ~4u));
        reply(p->chan, hdr.txid, 0, 0, 0, 0);
        break;
    }
    default:
        reply(p->chan, hdr.txid, -ENOSYS, 0, 0, 0);
    }
}

// ---- block devices ----

struct block {
    rx_block_info_t info;
    rx_handle_t server; // connection requests go here
};

static struct block blocks[MAX_BLOCK];
static int nblocks;

static int publish_block(const rx_block_info_t *in, rx_handle_t server, rx_block_info_t *out) {
    if (nblocks >= MAX_BLOCK) return -ENOSPC;
    struct block *b = &blocks[nblocks];
    b->info = *in;
    char prefix[24];
    snprintf(prefix, sizeof prefix, "%.20s", in->name[0] ? in->name : "blk");
    for (int n = 0;; n++) {
        snprintf(b->info.name, sizeof b->info.name, "%s%d", prefix, n);
        int taken = 0;
        for (int k = 0; k < nblocks; k++) taken |= !strcmp(blocks[k].info.name, b->info.name);
        if (!taken) break;
    }
    b->server = server;
    nblocks++;
    *out = b->info;
    return 0;
}

static void drop_block(int k) {
    rx_handle_close(blocks[k].server);
    blocks[k] = blocks[--nblocks];
}

// ---- /svc/devmgr clients ----

static rx_handle_t clients[MAX_CLIENTS];
static int nclients;

static void client_request(int c) {
    rx_handle_t ch = clients[c], h = 0;
    uint32_t nb = 0, nh = 0;
    rx_status_t s = rx_channel_read(ch, 0, req, &h, sizeof req, 1, &nb, &nh);
    if (s == RX_ERR_SHOULD_WAIT) return;
    if (s != RX_OK || nb < sizeof(rx_dev_hdr_t)) {
        if (nh) rx_handle_close(h);
        if (s != RX_OK) {
            rx_handle_close(ch);
            clients[c] = clients[--nclients];
        }
        return;
    }
    if (!nh) h = 0;
    rx_dev_hdr_t hdr;
    memcpy(&hdr, req, sizeof hdr);
    const char *body = req + sizeof hdr;
    size_t blen = nb - sizeof hdr;
    static char out[65536];
    switch (hdr.op) {
    case RX_DM_OP_LIST_PCI: {
        uint32_t n = 0;
        size_t off = 4;
        for (int k = 0; k < ndevs && off + sizeof(rx_pci_info_t) <= sizeof out - 64; k++) {
            rx_pci_info_t *i = &devs[k].info;
            i->command = R16(i, 4);
            i->status = R16(i, 6);
            memcpy(out + off, i, sizeof *i);
            off += sizeof *i;
            n++;
        }
        memcpy(out, &n, 4);
        reply(ch, hdr.txid, 0, out, off, 0);
        break;
    }
    case RX_DM_OP_LIST_BLOCK: {
        uint32_t n = (uint32_t)nblocks;
        memcpy(out, &n, 4);
        for (int k = 0; k < nblocks; k++) memcpy(out + 4 + k * sizeof(rx_block_info_t), &blocks[k].info, sizeof(rx_block_info_t));
        reply(ch, hdr.txid, 0, out, 4 + n * sizeof(rx_block_info_t), 0);
        break;
    }
    case RX_DM_OP_CONNECT_BLOCK: {
        char name[32] = { 0 };
        memcpy(name, body, blen < 31 ? blen : 31);
        int found = -1;
        for (int k = 0; k < nblocks; k++) if (!strcmp(blocks[k].info.name, name)) found = k;
        if (found < 0 || !h) {
            if (h) rx_handle_close(h);
            reply(ch, hdr.txid, h ? -ENOENT : -EINVAL, 0, 0, 0);
            break;
        }
        rx_svc_connect_t m = { RX_SVC_CONNECT, 0 };
        int st = rx_channel_write(blocks[found].server, 0, &m, sizeof m, &h, 1) == RX_OK ? 0 : -ENXIO;
        reply(ch, hdr.txid, st, 0, 0, 0);
        break;
    }
    case RX_DM_OP_PUBLISH_BLOCK: {
        rx_block_info_t in, res;
        if (blen < sizeof in || !h) {
            if (h) rx_handle_close(h);
            reply(ch, hdr.txid, -EINVAL, 0, 0, 0);
            break;
        }
        memcpy(&in, body, sizeof in);
        int st = publish_block(&in, h, &res);
        if (st < 0) rx_handle_close(h);
        reply(ch, hdr.txid, st, &res, sizeof res, 0);
        break;
    }
    default:
        if (h) rx_handle_close(h);
        reply(ch, hdr.txid, -ENOSYS, 0, 0, 0);
    }
}

// The kernel must refuse device-memory VMOs over RAM, or a driver could
// read anything. Check with a page that is RAM for certain: our own
// pinned buffer. Silent unless something is wrong.
static void ram_check(void) {
    rx_handle_t bti, vmo, pmt, phys;
    uint64_t pa = 0;
    if (rx_bti_create(resource, 0, 0xFFFF, &bti) != RX_OK) return;
    if (rx_vmo_create_contiguous(bti, 4096, 0, &vmo) == RX_OK &&
        rx_bti_pin(bti, RX_BTI_PERM_READ, vmo, 0, 4096, &pa, &pmt) == RX_OK) {
        rx_status_t s = rx_vmo_create_physical(resource, pa, 4096, &phys);
        if (s != RX_ERR_ACCESS_DENIED) {
            printf("devmgr: WARNING: the kernel gave a device-memory VMO over RAM at %#llx (status %lld)\n",
                   (unsigned long long)pa, (long long)s);
            if (s == RX_OK) rx_handle_close(phys);
        }
        rx_pmt_unpin(pmt);
        rx_handle_close(vmo);
    }
    rx_handle_close(bti);
}

// ---- main ----

int main(void) {
    resource = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    if (!resource) {
        fprintf(stderr, "devmgr: no root resource\n");
        return 1;
    }
    use_ecam = setup_ecam();
    if (!use_ecam && rx_ioports_request(resource, 0xCF8, 8) != RX_OK) {
        fprintf(stderr, "devmgr: no access to PCI configuration space\n");
        return 1;
    }
    scan_bus(use_ecam ? ecam_start : 0, 0);
    read_drivers();
    ram_check();

    rx_handle_t svc, svc_theirs;
    rx_channel_create(0, &svc, &svc_theirs);
    int r = rx_service_publish("/svc/devmgr", svc_theirs);
    if (r < 0) fprintf(stderr, "devmgr: cannot publish /svc/devmgr (%s)\n", strerror(-r));

    // Quiet when all is well (the shell is starting on the same screen);
    // lspci shows what was found
    for (int k = 0; k < ndevs; k++) {
        if (devs[k].prog[0]) start_driver(&devs[k]);
    }

    static rx_wait_item_t items[1 + MAX_CLIENTS + MAX_BLOCK + 2 * MAX_PCI];
    static int who[1 + MAX_CLIENTS + MAX_BLOCK + 2 * MAX_PCI]; // what each item is
    for (;;) {
        int n = 0;
        items[n] = (rx_wait_item_t){ svc, RX_CHANNEL_READABLE, 0 };
        who[n++] = 0;
        for (int c = 0; c < nclients; c++) {
            items[n] = (rx_wait_item_t){ clients[c], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
            who[n++] = 0x10000 | c;
        }
        for (int b = 0; b < nblocks; b++) {
            items[n] = (rx_wait_item_t){ blocks[b].server, RX_PEER_CLOSED, 0 };
            who[n++] = 0x20000 | b;
        }
        for (int k = 0; k < ndevs; k++) {
            if (devs[k].chan) {
                items[n] = (rx_wait_item_t){ devs[k].chan, RX_CHANNEL_READABLE, 0 };
                who[n++] = 0x30000 | k;
            }
            if (devs[k].proc) {
                items[n] = (rx_wait_item_t){ devs[k].proc, RX_PROCESS_TERMINATED, 0 };
                who[n++] = 0x40000 | k;
            }
        }
        if (rx_object_wait_many(items, (uint64_t)n, RX_TIME_INFINITE) != RX_OK) continue;
        // Handle from the end so removals do not shift what is still to do
        for (int k = n - 1; k >= 0; k--) {
            uint32_t pend = items[k].pending;
            int kind = who[k] >> 16, idx = who[k] & 0xFFFF;
            if (!pend) continue;
            if (k == 0) {
                rx_svc_connect_t m;
                rx_handle_t h = 0;
                uint32_t nb = 0, nh = 0;
                while (rx_channel_read(svc, 0, &m, &h, sizeof m, 1, &nb, &nh) == RX_OK) {
                    if (!nh) continue;
                    if (nclients < MAX_CLIENTS) clients[nclients++] = h;
                    else rx_handle_close(h);
                    nh = 0;
                }
            } else if (kind == 1) {
                if (idx < nclients) client_request(idx);
            } else if (kind == 2) {
                if (idx < nblocks && (pend & RX_PEER_CLOSED)) drop_block(idx);
            } else if (kind == 3) {
                if (devs[idx].chan && (pend & RX_CHANNEL_READABLE)) device_request(&devs[idx]);
            } else if (kind == 4) {
                if (devs[idx].proc) driver_ended(&devs[idx]);
            }
        }
    }
}
