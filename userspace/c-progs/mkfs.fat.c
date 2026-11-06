// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mkfs.fat: make a FAT filesystem on a block device (step 14). Everything
// on the device is lost.
//
//   mkfs.fat [-F 12|16|32] [-n LABEL] [-i SERIAL] [-s SECTORS] [-I] DEV
//
// Without -F the size decides: FAT12 below 8 MiB, FAT16 below 512 MiB,
// else FAT32. -s: sectors per cluster (a power of two; default as Windows
// formats). -I: use a whole disk that has a partition table. The layout
// follows Microsoft's FAT specification (1.03): two FATs, FAT32 with the
// FSInfo sector at 1 and the boot sector's backup at 6.

#include <errno.h>
#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static rx_handle_t ch;
static uint32_t bs;   // bytes per sector (the device's block size)

static void put16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

static int wr(uint64_t sector, uint32_t count, const void *buf) {
    int r = rx_block_write(ch, bs, sector, count, buf);
    if (r < 0) fprintf(stderr, "mkfs.fat: write at sector %llu: %s\n", (unsigned long long)sector, strerror(-r));
    return r;
}

static int zero(uint64_t sector, uint64_t count) {
    static uint8_t z[65536];
    uint32_t per = sizeof z / bs;
    while (count) {
        uint32_t n = count < per ? (uint32_t)count : per;
        if (wr(sector, n, z) < 0) return -1;
        sector += n;
        count -= n;
    }
    return 0;
}

static int mounted(const char *dev) {
    FILE *f = fopen("/etc/mtab", "r");
    if (!f) return 0;
    char line[600], want[80];
    snprintf(want, sizeof want, "/dev/%s ", dev);
    int m = 0;
    while (!m && fgets(line, sizeof line, f)) m = !strncmp(line, want, strlen(want));
    fclose(f);
    return m;
}

static void usage(void) {
    fprintf(stderr, "usage: mkfs.fat [-F 12|16|32] [-n LABEL] [-i SERIAL] [-s SECTORS] [-I] DEV\n");
}

int main(int argc, char **argv) {
    int type = 0, whole = 0, c;
    uint32_t spc = 0, serial = 0;
    int have_serial = 0;
    const char *label = 0;
    while ((c = getopt(argc, argv, "F:n:i:s:Ih")) != -1) {
        switch (c) {
        case 'F': type = atoi(optarg); break;
        case 'n': label = optarg; break;
        case 'i': serial = (uint32_t)strtoul(optarg, 0, 16); have_serial = 1; break;
        case 's': spc = (uint32_t)atoi(optarg); break;
        case 'I': whole = 1; break;
        default: usage(); return 2;
        }
    }
    if (optind != argc - 1 || (type && type != 12 && type != 16 && type != 32) ||
        (spc && (spc > 128 || (spc & (spc - 1))))) {
        usage();
        return 2;
    }
    const char *dev = argv[optind];
    if (!strncmp(dev, "/dev/", 5)) dev += 5;
    if (label && strlen(label) > 11) {
        fprintf(stderr, "mkfs.fat: a label has at most 11 characters\n");
        return 2;
    }
    rx_block_info_t info;
    int r = rx_block_open(dev, &ch);
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "mkfs.fat: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    if (info.flags & RX_BLOCK_FLAG_READONLY) {
        fprintf(stderr, "mkfs.fat: %s: read-only device\n", dev);
        return 1;
    }
    if (!(info.flags & RX_BLOCK_FLAG_PARTITION) && (info.flags & (RX_BLOCK_FLAG_GPT | RX_BLOCK_FLAG_MBR)) && !whole) {
        fprintf(stderr, "mkfs.fat: %s has a partition table: name a partition (or -I for the whole disk)\n", dev);
        return 1;
    }
    if (mounted(dev)) {
        fprintf(stderr, "mkfs.fat: %s is mounted (umount it first)\n", dev);
        return 1;
    }
    bs = info.block_size;
    if (bs != 512 && bs != 1024 && bs != 2048 && bs != 4096) {
        fprintf(stderr, "mkfs.fat: %s: sector size %u is not one FAT has\n", dev, bs);
        return 1;
    }
    uint64_t total64 = info.block_count;
    if (total64 > 0xFFFFFFFFull) total64 = 0xFFFFFFFFull;
    uint32_t total = (uint32_t)total64;
    uint64_t bytes = (uint64_t)total * bs;
    if (!type) type = bytes < (8ull << 20) ? 12 : bytes < (512ull << 20) ? 16 : 32;
    if (!spc) {
        // Windows' defaults, as bytes per cluster
        uint64_t cb;
        if (type == 32) cb = bytes <= (260ull << 20) ? 512 : bytes <= (8ull << 30) ? 4096 : bytes <= (16ull << 30) ? 8192 : bytes <= (32ull << 30) ? 16384 : 32768;
        else if (type == 16) cb = bytes <= (16ull << 20) ? 1024 : bytes <= (128ull << 20) ? 2048 : bytes <= (256ull << 20) ? 4096 : bytes <= (512ull << 20) ? 8192 : bytes <= (1ull << 30) ? 16384 : 32768;
        else cb = 512;
        spc = cb / bs ? (uint32_t)(cb / bs) : 1;
    }
    uint32_t reserved = type == 32 ? 32 : 1, nfats = 2, root_entries = type == 32 ? 0 : 512;
    uint32_t root_secs = (root_entries * 32 + bs - 1) / bs;
    uint32_t fatsz = 0, clusters = 0;
    for (;;) {
        // The FAT's size and the cluster count depend on each other: settle it
        fatsz = 1;
        for (int k = 0; k < 8; k++) {
            uint64_t data = (uint64_t)total - reserved - root_secs - (uint64_t)nfats * fatsz;
            if ((int64_t)data <= 0) { clusters = 0; break; }
            clusters = (uint32_t)(data / spc);
            uint64_t fb = type == 32 ? (clusters + 2ull) * 4 : type == 16 ? (clusters + 2ull) * 2 : ((clusters + 2ull) * 3 + 1) / 2;
            uint32_t need = (uint32_t)((fb + bs - 1) / bs);
            if (need == fatsz) break;
            fatsz = need;
        }
        // FAT12 has fewer than 4085 clusters; FAT16 4085 to 65524; FAT32 more
        if (type == 12 && clusters >= 4085 && spc < 128) { spc *= 2; continue; }
        if (type == 16 && clusters >= 65525 && spc < 128) { spc *= 2; continue; }
        if (type == 16 && clusters < 4085 && spc > 1) { spc /= 2; continue; }
        if (type == 32 && clusters < 65525 && spc > 1) { spc /= 2; continue; }
        break;
    }
    int ok = type == 12 ? clusters >= 1 && clusters < 4085 : type == 16 ? clusters >= 4085 && clusters < 65525 : clusters >= 65525 && clusters < 0x0FFFFFF5;
    if (!ok) {
        fprintf(stderr, "mkfs.fat: %s: %llu bytes is %s for FAT%d\n", dev, (unsigned long long)bytes,
                (type == 32 || (type == 16 && clusters < 4085)) ? "too small" : "too large", type);
        return 1;
    }
    if ((uint64_t)spc * bs > 65536) {
        fprintf(stderr, "mkfs.fat: clusters of %u bytes are too large\n", spc * bs);
        return 1;
    }
    if (!have_serial) {
        time_t t = time(0);
        serial = (uint32_t)t ^ (uint32_t)(rx_clock_get_monotonic() >> 10) * 2654435761u;
    }
    char lab[12];
    memset(lab, ' ', 11);
    lab[11] = 0;
    if (label) for (size_t i = 0; label[i]; i++) lab[i] = (char)(label[i] >= 'a' && label[i] <= 'z' ? label[i] - 32 : label[i]);
    else memcpy(lab, "NO NAME    ", 11);

    // The boot sector
    uint8_t *b = calloc(1, bs), *fs = calloc(1, bs);
    if (!b || !fs) return 1;
    b[0] = 0xEB;
    b[1] = type == 32 ? 0x58 : 0x3C;
    b[2] = 0x90;
    memcpy(b + 3, "RUSTUX  ", 8);
    put16(b + 11, bs);
    b[13] = (uint8_t)spc;
    put16(b + 14, reserved);
    b[16] = (uint8_t)nfats;
    put16(b + 17, root_entries);
    if (total < 65536 && type != 32) put16(b + 19, total);
    else put32(b + 32, total);
    b[21] = 0xF8;
    if (type != 32) put16(b + 22, fatsz);
    put16(b + 24, 63);   // sectors per track, heads: what BIOSes use for disks
    put16(b + 26, 255);
    put32(b + 28, (info.flags & RX_BLOCK_FLAG_PARTITION) ? (uint32_t)info.start : 0);
    uint8_t *ext = b + 36;
    if (type == 32) {
        put32(b + 36, fatsz);
        put16(b + 40, 0);    // flags: FATs mirrored
        put16(b + 42, 0);    // version 0.0
        put32(b + 44, 2);    // root directory: cluster 2
        put16(b + 48, 1);    // FSInfo
        put16(b + 50, 6);    // backup boot sector
        ext = b + 64;
    }
    ext[0] = 0x80;           // drive number
    ext[2] = 0x29;           // extended boot signature
    put32(ext + 3, serial);
    memcpy(ext + 7, lab, 11);
    memcpy(ext + 18, type == 32 ? "FAT32   " : type == 16 ? "FAT16   " : "FAT12   ", 8);
    // Boot code: a message for anyone who boots it
    static const char msg[] = "This is not a bootable disk.\r\n";
    uint8_t *code = ext + 26;
    size_t at = 0;
    uint32_t msg_off = (uint32_t)(code - b) + 32;
    // mov si, msg; lodsb; test al,al; jz hang; mov ah,0Eh; int 10h; jmp loop; hang: hlt; jmp hang
    static const uint8_t prog[] = { 0xBE, 0, 0, 0xAC, 0x84, 0xC0, 0x74, 0x06, 0xB4, 0x0E, 0xCD, 0x10, 0xEB, 0xF5, 0xF4, 0xEB, 0xFD };
    memcpy(code, prog, sizeof prog);
    put16(code + 1, 0x7C00 + msg_off);
    at = msg_off;
    memcpy(b + at, msg, sizeof msg);
    // The jump goes to the code
    b[1] = (uint8_t)((code - b) - 2);
    b[510] = 0x55;
    b[511] = 0xAA;
    if (bs > 512) { b[bs - 2] = 0x55; b[bs - 1] = 0xAA; }

    printf("mkfs.fat: %s: FAT%d, %u clusters of %u bytes, %llu MiB, serial %04X-%04X\n", dev, type, clusters, spc * bs,
           (unsigned long long)((uint64_t)clusters * spc * bs >> 20), serial >> 16, serial & 0xFFFF);
    fflush(stdout);

    // Clear the reserved area, the FATs, the root directory (and FAT32's
    // root cluster)
    uint64_t data_start = reserved + (uint64_t)nfats * fatsz + root_secs;
    if (zero(0, data_start + (type == 32 ? spc : 0)) < 0) return 1;
    // The first FAT entries: media, end-of-chain (and FAT32's root)
    uint8_t *f0 = calloc(1, bs);
    if (!f0) return 1;
    if (type == 32) {
        put32(f0, 0x0FFFFFF8);
        put32(f0 + 4, 0x0FFFFFFF);
        put32(f0 + 8, 0x0FFFFFFF);
    } else if (type == 16) {
        put16(f0, 0xFFF8);
        put16(f0 + 2, 0xFFFF);
    } else {
        f0[0] = 0xF8;
        f0[1] = 0xFF;
        f0[2] = 0xFF;
    }
    for (uint32_t k = 0; k < nfats; k++) if (wr(reserved + (uint64_t)k * fatsz, 1, f0) < 0) return 1;
    if (type == 32) {
        put32(fs, 0x41615252);
        put32(fs + 484, 0x61417272);
        put32(fs + 488, clusters - 1);   // the root directory has one
        put32(fs + 492, 3);
        put32(fs + 508, 0xAA550000);
        if (wr(1, 1, fs) < 0 || wr(7, 1, fs) < 0 || wr(6, 1, b) < 0) return 1;
    }
    if (label) {
        // The label as Windows shows it: an entry in the root directory
        uint8_t *d = calloc(1, bs);
        memcpy(d, lab, 11);
        d[11] = 0x08;
        time_t t = time(0);
        struct tm tm;
        localtime_r(&t, &tm);
        put16(d + 22, (uint32_t)(tm.tm_hour << 11 | tm.tm_min << 5 | tm.tm_sec / 2));
        put16(d + 24, (uint32_t)((tm.tm_year - 80) << 9 | (tm.tm_mon + 1) << 5 | tm.tm_mday));
        if (wr(type == 32 ? data_start : reserved + (uint64_t)nfats * fatsz, 1, d) < 0) return 1;
    }
    // The boot sector last: until it is there, nothing takes this for FAT
    if (wr(0, 1, b) < 0) return 1;
    r = rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
    if (r < 0) {
        fprintf(stderr, "mkfs.fat: %s: flush: %s\n", dev, strerror(-r));
        return 1;
    }
    return 0;
}
