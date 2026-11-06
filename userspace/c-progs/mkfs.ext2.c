// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mkfs.ext2: make an ext2 filesystem on a block device (step 15).
// Everything on the device is lost.
//
//   mkfs.ext2 [-b BLOCKSIZE] [-i BYTES-PER-INODE] [-L LABEL] [-U UUID]
//             [-m PERCENT] [-I] DEV
//
// As Linux's mke2fs makes ext2 (1.47), without the resize inode: revision
// 1, 256-byte inodes, features ext_attr, dir_index, filetype,
// sparse_super, large_file; 4 KiB blocks (1 KiB below 4 MiB), one inode
// per 16 KiB (4 KiB below 512 MiB), 5% reserved for root; a root directory
// and lost+found. -I: a whole disk that has a partition table.

#include <errno.h>
#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <time.h>
#include <unistd.h>

static rx_handle_t ch;
static uint32_t dev_bs, bs, spb;

static void put16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

static int wr(uint64_t block, uint32_t count, const void *buf) {
    int r = rx_block_write(ch, dev_bs, block * spb, count * spb, buf);
    if (r < 0) fprintf(stderr, "mkfs.ext2: write at block %llu: %s\n", (unsigned long long)block, strerror(-r));
    return r;
}

static int zero(uint64_t block, uint64_t count) {
    static uint8_t z[65536];
    uint32_t per = sizeof z / bs;
    while (count) {
        uint32_t n = count < per ? (uint32_t)count : per;
        if (wr(block, n, z) < 0) return -1;
        block += n;
        count -= n;
    }
    return 0;
}

// Groups with a superblock copy (sparse_super): 0, 1 and powers of 3, 5, 7
static int has_super(uint32_t g) {
    if (g <= 1) return 1;
    for (uint32_t b = 3; b <= 7; b += 2) {
        uint64_t p = b;
        while (p < g) p *= b;
        if (p == g) return 1;
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

static int parse_uuid(const char *s, uint8_t u[16]) {
    int n = 0;
    for (; *s && n < 32; s++) {
        if (*s == '-') continue;
        int v = *s >= '0' && *s <= '9' ? *s - '0' : *s >= 'a' && *s <= 'f' ? *s - 'a' + 10 : *s >= 'A' && *s <= 'F' ? *s - 'A' + 10 : -1;
        if (v < 0) return -1;
        if (n % 2 == 0) u[n / 2] = (uint8_t)(v << 4);
        else u[n / 2] |= (uint8_t)v;
        n++;
    }
    return n == 32 && !*s ? 0 : -1;
}

static void usage(void) {
    fprintf(stderr, "usage: mkfs.ext2 [-b BLOCKSIZE] [-i BYTES-PER-INODE] [-L LABEL] [-U UUID] [-m PERCENT] [-I] DEV\n");
}

int main(int argc, char **argv) {
    uint32_t want_bs = 0, ratio = 0, reserve_pct = 5;
    const char *label = "", *uuid_s = 0;
    int whole = 0, c;
    while ((c = getopt(argc, argv, "b:i:L:U:m:Ih")) != -1) {
        switch (c) {
        case 'b': want_bs = (uint32_t)atoi(optarg); break;
        case 'i': ratio = (uint32_t)atoi(optarg); break;
        case 'L': label = optarg; break;
        case 'U': uuid_s = optarg; break;
        case 'm': reserve_pct = (uint32_t)atoi(optarg); break;
        case 'I': whole = 1; break;
        default: usage(); return 2;
        }
    }
    if (optind != argc - 1 || (want_bs && want_bs != 1024 && want_bs != 2048 && want_bs != 4096) || reserve_pct > 50) {
        usage();
        return 2;
    }
    if (strlen(label) > 16) {
        fprintf(stderr, "mkfs.ext2: a label has at most 16 characters\n");
        return 2;
    }
    uint8_t uuid[16];
    if (uuid_s ? parse_uuid(uuid_s, uuid) < 0 : getrandom(uuid, 16, 0) != 16) {
        fprintf(stderr, "mkfs.ext2: %s\n", uuid_s ? "a UUID is 32 hex digits" : "no random numbers");
        return 2;
    }
    if (!uuid_s) {
        uuid[6] = (uint8_t)((uuid[6] & 0x0F) | 0x40);   // version 4
        uuid[8] = (uint8_t)((uuid[8] & 0x3F) | 0x80);
    }
    const char *dev = argv[optind];
    if (!strncmp(dev, "/dev/", 5)) dev += 5;
    rx_block_info_t info;
    int r = rx_block_open(dev, &ch);
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "mkfs.ext2: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    if (info.flags & RX_BLOCK_FLAG_READONLY) {
        fprintf(stderr, "mkfs.ext2: %s: read-only device\n", dev);
        return 1;
    }
    if (!(info.flags & RX_BLOCK_FLAG_PARTITION) && (info.flags & (RX_BLOCK_FLAG_GPT | RX_BLOCK_FLAG_MBR)) && !whole) {
        fprintf(stderr, "mkfs.ext2: %s has a partition table: name a partition (or -I for the whole disk)\n", dev);
        return 1;
    }
    if (mounted(dev)) {
        fprintf(stderr, "mkfs.ext2: %s is mounted (umount it first)\n", dev);
        return 1;
    }
    dev_bs = info.block_size;
    uint64_t bytes = info.block_count * dev_bs;
    bs = want_bs ? want_bs : bytes < (4ull << 20) ? 1024 : 4096;
    if (bs < dev_bs) bs = dev_bs;
    if (bs > 4096) {
        fprintf(stderr, "mkfs.ext2: %s: sectors of %u bytes are too large\n", dev, dev_bs);
        return 1;
    }
    spb = bs / dev_bs;
    if (!ratio) ratio = bytes < (512ull << 20) ? 4096 : 16384;
    if (ratio < bs) ratio = bs;
    uint64_t nb64 = bytes / bs;
    if (nb64 > 0xFFFFFFFFull) nb64 = 0xFFFFFFFFull;
    uint32_t nblocks = (uint32_t)nb64, first_data = bs == 1024 ? 1 : 0, bpg = bs * 8;
    if (nblocks < 64) {
        fprintf(stderr, "mkfs.ext2: %s is too small\n", dev);
        return 1;
    }
    uint32_t ngroups = (nblocks - first_data + bpg - 1) / bpg;
    // A last group too small for its own metadata is dropped
    uint32_t isz = 256;
    uint64_t want_inodes = bytes / ratio;
    uint32_t ipb = bs / isz;
    uint32_t ipg = (uint32_t)((want_inodes + ngroups - 1) / ngroups);
    // Whole inode-table blocks and whole bitmap bytes
    uint32_t unit = ipb > 8 ? ipb : 8;
    if (ipg < 16) ipg = 16;
    ipg = (ipg + unit - 1) / unit * unit;
    if (ipg > bs * 8) ipg = bs * 8;
    uint32_t itb = ipg / ipb;                                // inode-table blocks per group
    uint32_t gdt_blocks = (ngroups * 32 + bs - 1) / bs;
    for (;;) {
        uint32_t last = nblocks - first_data - (ngroups - 1) * bpg;
        uint32_t overhead = (has_super(ngroups - 1) ? 1 + gdt_blocks : 0) + 2 + itb;
        if (ngroups > 1 && last < overhead + 50) {
            nblocks -= last;
            ngroups--;
            gdt_blocks = (ngroups * 32 + bs - 1) / bs;
            continue;
        }
        break;
    }
    uint32_t ninodes = ipg * ngroups;

    // Layout per group: [superblock, GDT] block bitmap, inode bitmap, inode table, data
    uint32_t *bbm = calloc(ngroups, 4), *ibm = calloc(ngroups, 4), *itab = calloc(ngroups, 4), *gfree = calloc(ngroups, 4);
    uint8_t *gdt = calloc(gdt_blocks, bs), *buf = calloc(1, bs), *sbb = calloc(1, 1024);
    if (!bbm || !ibm || !itab || !gfree || !gdt || !buf || !sbb) return 1;
    for (uint32_t g = 0; g < ngroups; g++) {
        uint32_t start = first_data + g * bpg;
        uint32_t at = start + (has_super(g) ? 1 + gdt_blocks : 0);
        bbm[g] = at;
        ibm[g] = at + 1;
        itab[g] = at + 2;
    }
    // Root directory (inode 2) and lost+found (11): blocks after group 0's table
    uint32_t data0 = itab[0] + itb;
    // lost+found gets 16 KiB (12 KiB with 1 KiB blocks), so fsck can use it
    uint32_t root_blk = data0, lf_blk = data0 + 1, lf_nblocks = bs == 1024 ? 12 : 16384 / bs;
    uint32_t used0_end = lf_blk + lf_nblocks;   // group 0 blocks used up to here
    uint32_t total_free = 0;

    time_t now = time(0);
    printf("mkfs.ext2: %s: %u blocks of %u bytes, %u inodes, %u groups\n", dev, nblocks, bs, ninodes, ngroups);
    fflush(stdout);

    // Groups: bitmaps, empty inode tables
    for (uint32_t g = 0; g < ngroups; g++) {
        uint32_t start = first_data + g * bpg;
        uint32_t in_group = g == ngroups - 1 ? nblocks - start : bpg;
        uint32_t used_to = g == 0 ? used0_end : itab[g] + itb;   // blocks start..used_to-1 are in use
        memset(buf, 0, bs);
        for (uint32_t i = 0; i < used_to - start; i++) buf[i / 8] |= (uint8_t)(1u << (i % 8));
        for (uint32_t i = in_group; i < bs * 8; i++) buf[i / 8] |= (uint8_t)(1u << (i % 8));   // past the end
        if (wr(bbm[g], 1, buf) < 0) return 1;
        gfree[g] = in_group - (used_to - start);
        total_free += gfree[g];
        memset(buf, 0, bs);
        if (g == 0) for (uint32_t i = 0; i < 11; i++) buf[i / 8] |= (uint8_t)(1u << (i % 8));   // inodes 1-11
        for (uint32_t i = ipg; i < bs * 8; i++) buf[i / 8] |= (uint8_t)(1u << (i % 8));
        if (wr(ibm[g], 1, buf) < 0) return 1;
        if (zero(itab[g], itb) < 0) return 1;
        uint8_t *d = gdt + g * 32;
        put32(d, bbm[g]);
        put32(d + 4, ibm[g]);
        put32(d + 8, itab[g]);
        put16(d + 12, gfree[g]);
        put16(d + 14, g == 0 ? ipg - 11 : ipg);
        put16(d + 16, g == 0 ? 2 : 0);
    }

    // Inodes: 2 (root) and 11 (lost+found) in group 0's table
    uint8_t *ib = calloc(1, bs);
    if (!ib) return 1;
    for (int which = 0; which < 2; which++) {
        uint32_t ino = which ? 11 : 2;
        uint32_t idx = ino - 1, blk = itab[0] + idx * isz / bs, off = idx * isz % bs;
        memset(ib, 0, bs);
        if (rx_block_read(ch, dev_bs, (uint64_t)blk * spb, spb, ib) < 0) return 1;
        uint8_t *p = ib + off;
        memset(p, 0, isz);
        put16(p, which ? 040700 : 040755);
        uint32_t nblk = which ? lf_nblocks : 1;
        put32(p + 4, nblk * bs);
        put32(p + 8, (uint32_t)now);
        put32(p + 12, (uint32_t)now);
        put32(p + 16, (uint32_t)now);
        put16(p + 26, which ? 2 : 3);   // root: ".", "..", lost+found's ".."
        put32(p + 28, nblk * (bs / 512));
        for (uint32_t k = 0; k < nblk; k++) put32(p + 40 + 4 * k, (which ? lf_blk : root_blk) + k);
        put16(p + 128, 32);
        put32(p + 144, (uint32_t)now);   // created
        if (wr(blk, 1, ib) < 0) return 1;
    }
    // Their directory blocks
    memset(buf, 0, bs);
    put32(buf, 2); put16(buf + 4, 12); buf[6] = 1; buf[7] = 2; buf[8] = '.';
    put32(buf + 12, 2); put16(buf + 16, 12); buf[18] = 2; buf[19] = 2; buf[20] = '.'; buf[21] = '.';
    put32(buf + 24, 11); put16(buf + 28, bs - 24); buf[30] = 10; buf[31] = 2; memcpy(buf + 32, "lost+found", 10);
    if (wr(root_blk, 1, buf) < 0) return 1;
    memset(buf, 0, bs);
    put32(buf, 11); put16(buf + 4, 12); buf[6] = 1; buf[7] = 2; buf[8] = '.';
    put32(buf + 12, 2); put16(buf + 16, bs - 12); buf[18] = 2; buf[19] = 2; buf[20] = '.'; buf[21] = '.';
    if (wr(lf_blk, 1, buf) < 0) return 1;
    memset(buf, 0, bs);
    put16(buf + 4, bs);   // empty blocks: one unused entry
    for (uint32_t k = 1; k < lf_nblocks; k++) if (wr(lf_blk + k, 1, buf) < 0) return 1;

    // The superblock
    uint8_t *s = sbb;
    put32(s, ninodes);
    put32(s + 4, nblocks);
    put32(s + 8, (uint32_t)((uint64_t)nblocks * reserve_pct / 100));
    put32(s + 12, total_free);
    put32(s + 16, ninodes - 11);
    put32(s + 20, first_data);
    put32(s + 24, bs == 1024 ? 0 : bs == 2048 ? 1 : 2);
    put32(s + 28, bs == 1024 ? 0 : bs == 2048 ? 1 : 2);
    put32(s + 32, bpg);
    put32(s + 36, bpg);
    put32(s + 40, ipg);
    put32(s + 48, (uint32_t)now);          // write time
    put16(s + 54, 0xFFFF);                 // no forced checks by mount count
    put16(s + 56, 0xEF53);
    put16(s + 58, 1);                      // clean
    put16(s + 60, 1);                      // on errors: continue
    put32(s + 64, (uint32_t)now);          // last check
    put32(s + 72, 0);                      // creator: Linux (as e2fsprogs expects)
    put32(s + 76, 1);                      // dynamic revision
    put32(s + 84, 11);                     // first non-reserved inode
    put16(s + 88, isz);
    put32(s + 92, 0x0008 | 0x0020);        // ext_attr, dir_index
    put32(s + 96, 0x0002);                 // filetype
    put32(s + 100, 0x0001 | 0x0002);       // sparse_super, large_file
    memcpy(s + 104, uuid, 16);
    memcpy(s + 120, label, strlen(label));
    uint32_t seed[4];
    if (getrandom(seed, sizeof seed, 0) != sizeof seed) memcpy(seed, uuid, 16);
    memcpy(s + 236, seed, 16);             // directory hash seed
    s[252] = 1;                            // half_md4
    put32(s + 264, (uint32_t)now);         // mkfs time
    put16(s + 348, 32);                    // inode extra sizes
    put16(s + 350, 32);
    put32(s + 352, 1);                     // signed directory hash
    // Clear the start (boot sectors, old signatures), then the copies
    if (zero(0, first_data + 1 + gdt_blocks) < 0) return 1;
    for (uint32_t g = 0; g < ngroups; g++) {
        if (!has_super(g)) continue;
        uint32_t start = first_data + g * bpg;
        put16(s + 90, g);                  // which group this copy is in
        if (g == 0) {
            // At byte 1024
            uint8_t *b0 = calloc(1, bs > 2048 ? bs : 2048);
            if (!b0) return 1;
            memcpy(b0 + 1024, s, 1024);
            if (bs == 1024) { if (wr(1, 1, b0 + 1024) < 0) return 1; }
            else if (wr(0, 1, b0) < 0) return 1;
            free(b0);
        } else {
            memset(buf, 0, bs);
            memcpy(buf, s, 1024);
            if (wr(start, 1, buf) < 0) return 1;
        }
        if (wr(start + 1, gdt_blocks, gdt) < 0) return 1;
    }
    r = rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
    if (r < 0) {
        fprintf(stderr, "mkfs.ext2: %s: flush: %s\n", dev, strerror(-r));
        return 1;
    }
    printf("mkfs.ext2: UUID %02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x\n", uuid[0], uuid[1],
           uuid[2], uuid[3], uuid[4], uuid[5], uuid[6], uuid[7], uuid[8], uuid[9], uuid[10], uuid[11], uuid[12], uuid[13],
           uuid[14], uuid[15]);
    return 0;
}
