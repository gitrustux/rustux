// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// lspci: list PCI devices, as devmgr found them (/svc/devmgr).
//
//   lspci [-n | -nn] [-v] [-k] [-D] [-s [[bus]:][dev][.func]] [-d [vendor]:[device]] [--json]
//
// Names come from a small built-in table of vendors and the PCI class
// codes (there is no pci.ids database yet).

#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const struct { uint16_t id; const char *name; } vendors[] = {
    { 0x8086, "Intel Corporation" }, { 0x8087, "Intel Corp." }, { 0x1022, "Advanced Micro Devices, Inc. [AMD]" },
    { 0x1002, "Advanced Micro Devices, Inc. [AMD/ATI]" }, { 0x10de, "NVIDIA Corporation" },
    { 0x1af4, "Red Hat, Inc." }, { 0x1b36, "Red Hat, Inc." }, { 0x10ec, "Realtek Semiconductor Co., Ltd." },
    { 0x144d, "Samsung Electronics Co Ltd" }, { 0x15b7, "Sandisk Corp" }, { 0x1c5c, "SK hynix" },
    { 0x14e4, "Broadcom Inc. and subsidiaries" }, { 0x168c, "Qualcomm Atheros" }, { 0x17cb, "Qualcomm Technologies, Inc" },
    { 0x1987, "Phison Electronics Corporation" }, { 0x1e0f, "KIOXIA Corporation" }, { 0x126f, "Silicon Motion, Inc." },
    { 0x1179, "Toshiba Corporation" }, { 0x2646, "Kingston Technology Company, Inc." }, { 0x1b21, "ASMedia Technology Inc." },
    { 0x1106, "VIA Technologies, Inc." }, { 0x1013, "Cirrus Logic" }, { 0x15ad, "VMware" }, { 0x80ee, "InnoTek Systemberatung GmbH" },
    { 0x1414, "Microsoft Corporation" }, { 0x1d0f, "Amazon.com, Inc." }, { 0x1b4b, "Marvell Technology Group Ltd." },
    { 0x11ab, "Marvell Technology Group Ltd." }, { 0x14c3, "MEDIATEK Corp." }, { 0x1912, "Renesas Technology Corp." },
    { 0x1217, "O2 Micro, Inc." }, { 0x1180, "Ricoh Co Ltd" }, { 0x10b5, "PLX Technology, Inc." }, { 0x1000, "Broadcom / LSI" },
    { 0x1d6a, "Aquantia Corp." }, { 0x15b3, "Mellanox Technologies" }, { 0x1c5f, "Beijing Memblaze Technology Co. Ltd." },
    { 0x1cc1, "ADATA Technology Co., Ltd." }, { 0x1dbe, "INNOGRIT Corporation" }, { 0x1bb1, "Seagate Technology PLC" },
    { 0x1c58, "HGST, Inc." }, { 0x1344, "Micron Technology Inc" }, { 0x1e95, "Solid State Storage Technology Corporation" },
};

static const char *vendor_name(uint16_t id) {
    for (size_t k = 0; k < sizeof vendors / sizeof *vendors; k++) if (vendors[k].id == id) return vendors[k].name;
    return 0;
}

static const struct { uint8_t c, s; const char *name; } classes[] = {
    { 0x00, 0x00, "Non-VGA unclassified device" }, { 0x00, 0x01, "VGA compatible unclassified device" },
    { 0x01, 0x00, "SCSI storage controller" }, { 0x01, 0x01, "IDE interface" }, { 0x01, 0x02, "Floppy disk controller" },
    { 0x01, 0x04, "RAID bus controller" }, { 0x01, 0x05, "ATA controller" }, { 0x01, 0x06, "SATA controller" },
    { 0x01, 0x07, "Serial Attached SCSI controller" }, { 0x01, 0x08, "Non-Volatile memory controller" },
    { 0x01, 0x80, "Mass storage controller" },
    { 0x02, 0x00, "Ethernet controller" }, { 0x02, 0x80, "Network controller" },
    { 0x03, 0x00, "VGA compatible controller" }, { 0x03, 0x01, "XGA compatible controller" }, { 0x03, 0x02, "3D controller" },
    { 0x03, 0x80, "Display controller" },
    { 0x04, 0x00, "Multimedia video controller" }, { 0x04, 0x01, "Multimedia audio controller" },
    { 0x04, 0x03, "Audio device" }, { 0x04, 0x80, "Multimedia controller" },
    { 0x05, 0x00, "RAM memory" }, { 0x05, 0x01, "FLASH memory" }, { 0x05, 0x80, "Memory controller" },
    { 0x06, 0x00, "Host bridge" }, { 0x06, 0x01, "ISA bridge" }, { 0x06, 0x04, "PCI bridge" }, { 0x06, 0x07, "CardBus bridge" },
    { 0x06, 0x80, "Bridge" },
    { 0x07, 0x00, "Serial controller" }, { 0x07, 0x01, "Parallel controller" }, { 0x07, 0x03, "Modem" },
    { 0x07, 0x80, "Communication controller" },
    { 0x08, 0x00, "PIC" }, { 0x08, 0x01, "DMA controller" }, { 0x08, 0x02, "Timer" }, { 0x08, 0x03, "RTC" },
    { 0x08, 0x05, "SD Host controller" }, { 0x08, 0x06, "IOMMU" }, { 0x08, 0x80, "System peripheral" },
    { 0x09, 0x00, "Keyboard controller" }, { 0x09, 0x02, "Mouse controller" }, { 0x09, 0x80, "Input device controller" },
    { 0x0c, 0x00, "FireWire (IEEE 1394)" }, { 0x0c, 0x03, "USB controller" }, { 0x0c, 0x04, "Fibre Channel" },
    { 0x0c, 0x05, "SMBus" }, { 0x0c, 0x07, "IPMI Interface" }, { 0x0c, 0x80, "Serial bus controller" },
    { 0x0d, 0x11, "Bluetooth" }, { 0x0d, 0x80, "Wireless controller" },
    { 0x10, 0x00, "Network and computing encryption device" }, { 0x10, 0x80, "Encryption controller" },
    { 0x11, 0x80, "Signal processing controller" },
};

static const char *class_top[] = {
    "Unclassified device", "Mass storage controller", "Network controller", "Display controller",
    "Multimedia controller", "Memory controller", "Bridge", "Communication controller", "Generic system peripheral",
    "Input device controller", "Docking station", "Processor", "Serial bus controller", "Wireless controller",
    "Intelligent controller", "Satellite communications controller", "Encryption controller",
    "Signal processing controller", "Processing accelerators", "Non-Essential Instrumentation",
};

static const char *class_name(uint8_t c, uint8_t s) {
    for (size_t k = 0; k < sizeof classes / sizeof *classes; k++) if (classes[k].c == c && classes[k].s == s) return classes[k].name;
    if (c < sizeof class_top / sizeof *class_top) return class_top[c];
    if (c == 0x40) return "Coprocessor";
    if (c == 0xff) return "Unassigned class";
    return "Unknown class";
}

static void size_str(uint64_t n, char *buf, size_t cap) {
    const char *u = "KMGT";
    if (n < 1024) { snprintf(buf, cap, "%llu", (unsigned long long)n); return; }
    int k = -1;
    while (n >= 1024 && n % 1024 == 0 && k < 3) { n /= 1024; k++; }
    if (k < 0) snprintf(buf, cap, "%llu", (unsigned long long)n);
    else snprintf(buf, cap, "%llu%c", (unsigned long long)n, u[k]);
}

static void json_str(const char *s) {
    if (!s) { fputs("null", stdout); return; }
    putchar('"');
    for (; *s; s++) {
        if (*s == '"' || *s == '\\') putchar('\\');
        putchar(*s);
    }
    putchar('"');
}

static void usage(FILE *f) {
    fprintf(f, "usage: lspci [-n|-nn] [-v] [-k] [-D] [-s [[bus]:][dev][.func]] [-d [vendor]:[device]] [--json]\n"
               "List the PCI devices the device manager found.\n"
               "  -n       numbers instead of names (-nn: both)\n"
               "  -v       details: subsystem, BARs, interrupts, capabilities, driver\n"
               "  -k       the driver of each device\n"
               "  -D       show the PCI domain (0000:)\n"
               "  -s SLOT  only devices at bus:dev.func (any part may be left out)\n"
               "  -d ID    only devices with vendor:device (hex; either may be left out)\n"
               "  --json   a JSON array\n");
}

// -s [[bus]:][dev][.func]
static int slot_match(const char *s, const rx_pci_info_t *i) {
    const char *colon = strchr(s, ':'), *dot = strchr(s, '.');
    char *end;
    if (colon) {
        if (colon > s && strtoul(s, &end, 16) != i->bus) return 0;
        s = colon + 1;
    }
    if (*s && s != dot && strtoul(s, &end, 16) != i->dev) return 0;
    if (dot && dot[1] && strtoul(dot + 1, &end, 16) != i->func) return 0;
    return 1;
}

static int id_match(const char *s, const rx_pci_info_t *i) {
    const char *colon = strchr(s, ':');
    char *end;
    if (s != colon && *s && strtoul(s, &end, 16) != i->vendor) return 0;
    if (colon && colon[1] && strtoul(colon + 1, &end, 16) != i->device) return 0;
    return 1;
}

int main(int argc, char **argv) {
    int numeric = 0, verbose = 0, kernel = 0, domain = 0, json = 0;
    const char *slot = 0, *ids = 0;
    for (int a = 1; a < argc; a++) {
        const char *s = argv[a];
        if (!strcmp(s, "--json") || !strcmp(s, "-J")) json = 1;
        else if (!strcmp(s, "--help") || !strcmp(s, "-h")) { usage(stdout); return 0; }
        else if (!strcmp(s, "-s") && a + 1 < argc) slot = argv[++a];
        else if (!strcmp(s, "-d") && a + 1 < argc) ids = argv[++a];
        else if (s[0] == '-' && s[1] && s[1] != '-') {
            for (const char *c = s + 1; *c; c++) {
                if (*c == 'n') numeric++;
                else if (*c == 'v') verbose++;
                else if (*c == 'k') kernel = 1;
                else if (*c == 'D') domain = 1;
                else { fprintf(stderr, "lspci: unknown option -%c\n", *c); usage(stderr); return 2; }
            }
        } else { fprintf(stderr, "lspci: unknown argument %s\n", s); usage(stderr); return 2; }
    }
    rx_handle_t dm;
    int r = rx_devmgr_connect(&dm);
    if (r < 0) {
        fprintf(stderr, "lspci: cannot reach the device manager (%s)\n", strerror(-r));
        return 1;
    }
    static char buf[65536];
    size_t got = 0;
    r = rx_dev_call(dm, RX_DM_OP_LIST_PCI, 0, 0, 0, buf, sizeof buf, &got, 0);
    if (r < 0 || got < 4) {
        fprintf(stderr, "lspci: the device manager did not answer (%s)\n", strerror(r < 0 ? -r : 5));
        return 1;
    }
    uint32_t n;
    memcpy(&n, buf, 4);
    if (json) putchar('[');
    int first = 1;
    for (uint32_t k = 0; k < n && 4 + (k + 1) * sizeof(rx_pci_info_t) <= got; k++) {
        rx_pci_info_t d;
        memcpy(&d, buf + 4 + k * sizeof d, sizeof d);
        if ((slot && !slot_match(slot, &d)) || (ids && !id_match(ids, &d))) continue;
        char where[24];
        snprintf(where, sizeof where, "%s%02x:%02x.%x", domain || json ? "0000:" : "", d.bus, d.dev, d.func);
        const char *vn = vendor_name(d.vendor), *cn = class_name(d.class_code, d.subclass);
        if (json) {
            printf("%s\n  {\"slot\":\"%s\",\"class\":\"%02x%02x\",\"prog_if\":\"%02x\",\"class_name\":", first ? "" : ",", where,
                   d.class_code, d.subclass, d.prog_if);
            json_str(cn);
            printf(",\"vendor\":\"%04x\",\"vendor_name\":", d.vendor);
            json_str(vn);
            printf(",\"device\":\"%04x\",\"revision\":%u,\"subsystem_vendor\":\"%04x\",\"subsystem\":\"%04x\",",
                   d.device, d.revision, d.subsystem_vendor, d.subsystem);
            printf("\"driver\":");
            json_str(d.driver[0] ? d.driver : 0);
            printf(",\"driver_pid\":%llu,\"restarts\":%u,\"kernel\":%s,\"msi\":%s,\"msix_vectors\":%u,\"pcie\":%s,\"bars\":[",
                   (unsigned long long)d.driver_koid, d.restarts, d.flags & RX_PCI_FLAG_KERNEL ? "true" : "false",
                   d.msi ? "true" : "false", d.msix ? d.msix_count : 0, d.pcie ? "true" : "false");
            int fb = 1;
            for (int b = 0; b < 6; b++) {
                rx_pci_bar_t *bar = &d.bars[b];
                if (bar->type == RX_PCI_BAR_NONE) continue;
                printf("%s{\"index\":%d,\"type\":\"%s\",\"address\":%llu,\"size\":%llu,\"prefetchable\":%s,\"64bit\":%s}",
                       fb ? "" : ",", b, bar->type == RX_PCI_BAR_IO ? "io" : "memory", (unsigned long long)bar->address,
                       (unsigned long long)bar->size, bar->flags & RX_PCI_BAR_PREFETCH ? "true" : "false",
                       bar->flags & RX_PCI_BAR_64BIT ? "true" : "false");
                fb = 0;
            }
            printf("]}");
            first = 0;
            continue;
        }
        printf("%s ", where);
        if (numeric == 1) {
            printf("%02x%02x: %04x:%04x", d.class_code, d.subclass, d.vendor, d.device);
        } else {
            printf("%s", cn);
            if (numeric >= 2) printf(" [%02x%02x]", d.class_code, d.subclass);
            printf(": ");
            if (vn) printf("%s ", vn);
            else printf("Vendor %04x ", d.vendor);
            printf("Device %04x", d.device);
            if (numeric >= 2) printf(" [%04x:%04x]", d.vendor, d.device);
        }
        if (d.revision) printf(" (rev %02x)", d.revision);
        if (d.prog_if && verbose) printf(" (prog-if %02x)", d.prog_if);
        putchar('\n');
        if (verbose) {
            if (d.subsystem_vendor || d.subsystem) {
                const char *sv = vendor_name(d.subsystem_vendor);
                if (sv && numeric != 1) printf("\tSubsystem: %s Device %04x\n", sv, d.subsystem);
                else printf("\tSubsystem: %04x:%04x\n", d.subsystem_vendor, d.subsystem);
            }
            printf("\tFlags:%s%s%s", d.command & 4 ? " bus master," : "", d.command & 0x400 ? " INTx disabled," : "",
                   d.irq_pin ? "" : " no INTx,");
            if (d.irq_pin) printf(" INT%c (firmware line %u)", 'A' + d.irq_pin - 1, d.irq_line);
            putchar('\n');
            for (int b = 0; b < 6; b++) {
                rx_pci_bar_t *bar = &d.bars[b];
                if (bar->type == RX_PCI_BAR_NONE) continue;
                char sz[24];
                size_str(bar->size, sz, sizeof sz);
                if (bar->type == RX_PCI_BAR_IO) {
                    printf("\tI/O ports at %llx", (unsigned long long)bar->address);
                } else {
                    printf("\tMemory at %llx (%s, %sprefetchable)", (unsigned long long)bar->address,
                           bar->flags & RX_PCI_BAR_64BIT ? "64-bit" : "32-bit", bar->flags & RX_PCI_BAR_PREFETCH ? "" : "non-");
                }
                if (bar->size) printf(" [size=%s]", sz);
                else printf(" [size not read: in use by the kernel]");
                putchar('\n');
            }
            if (d.msi || d.msix || d.pcie) {
                printf("\tCapabilities:");
                const char *sep = " ";
                if (d.pcie) { printf("%sPCI Express", sep); sep = ", "; }
                if (d.msi) { printf("%sMSI (%u vector%s)", sep, d.msi_count, d.msi_count == 1 ? "" : "s"); sep = ", "; }
                if (d.msix) printf("%sMSI-X (%u vectors)", sep, d.msix_count);
                putchar('\n');
            }
        }
        if (kernel || verbose) {
            if (d.flags & RX_PCI_FLAG_KERNEL) printf("\tKernel driver in use: kernel (left to the kernel)\n");
            else if (d.driver[0] && d.driver_koid) printf("\tDriver in use: %s (process %llu%s)\n", d.driver,
                                                         (unsigned long long)d.driver_koid, d.restarts ? ", restarted" : "");
            else if (d.driver[0]) printf("\tDriver: %s (not running)\n", d.driver);
        }
        if (verbose) putchar('\n');
    }
    if (json) printf("%s]\n", first ? "" : "\n");
    return 0;
}
