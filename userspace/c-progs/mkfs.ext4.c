// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// mkfs.ext4: make an ext4 filesystem on a block device (step 16).
// Everything on the device is lost.
//
//   mkfs.ext4 [-b BLOCKSIZE] [-i BYTES-PER-INODE] [-J size=MiB] [-L LABEL]
//             [-U UUID] [-m PERCENT] [-O ^has_journal] [-I] DEV
//
// As Linux's mke2fs makes ext4 (1.47), without the resize inode: 256-byte
// inodes, features has_journal ext_attr dir_index filetype extent 64bit
// flex_bg sparse_super large_file huge_file dir_nlink extra_isize
// metadata_csum; 4 KiB blocks (1 KiB below 4 MiB), one inode per 16 KiB
// (4 KiB below 512 MiB), 5% reserved for root; groups in flexible groups
// of 16 (their bitmaps and inode tables together); a journal of 4 to 512
// MiB (by size, as mke2fs picks); a root directory and lost+found. Groups
// nothing uses yet are left "uninit" (their bitmaps made when first
// needed); only the first inode table is cleared. -I: a whole disk that
// has a partition table.

#include <errno.h>
#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <time.h>
#include <unistd.h>

#include "ext4lib.h"

static rx_handle_t ch;
static uint32_t dev_bs, bs, spb;

static int wr(uint64_t block, uint32_t count, const void *buf) {
    int r = rx_block_write(ch, dev_bs, block * spb, count * spb, buf);
    if (r < 0) fprintf(stderr, "mkfs.ext4: write at block %llu: %s\n", (unsigned long long)block, strerror(-r));
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
    fprintf(stderr, "usage: mkfs.ext4 [-b BLOCKSIZE] [-i BYTES-PER-INODE] [-J size=MiB] [-L LABEL] [-U UUID] [-m PERCENT]\n"
                    "                 [-O ^has_journal] [-I] DEV\n");
}

// The whole filesystem's block bitmap while it is laid out
static uint8_t *used;
static uint32_t nblocks;
static int is_used(uint32_t b) { return (used[b / 8] >> (b % 8)) & 1; }
static void use(uint32_t b, uint32_t n) { for (uint32_t i = b; i < b + n; i++) used[i / 8] |= (uint8_t)(1u << (i % 8)); }

// n free blocks in a row, at or after `from`
static uint32_t take(uint32_t *from, uint32_t n) {
    for (uint32_t b = *from; b + n <= nblocks;) {
        uint32_t k = 0;
        while (k < n && !is_used(b + k)) k++;
        if (k == n) {
            use(b, n);
            *from = b + n;
            return b;
        }
        b += k + 1;
    }
    return 0;
}

// The default journal, in blocks (as mke2fs's ext2fs_default_journal_size)
static uint32_t journal_blocks(uint64_t blocks) {
    if (blocks < 2048) return 0;
    if (blocks < 32768) return 1024;
    if (blocks < 256 * 1024) return 4096;
    if (blocks < 512 * 1024) return 8192;
    if (blocks < 4096 * 1024) return 16384;
    if (blocks < 8192 * 1024) return 32768;
    if (blocks < 16384 * 1024) return 65536;
    return 131072;   // mke2fs goes to 262144 past 128 GiB; here at most 4 extents in the inode
}

int main(int argc, char **argv) {
    uint32_t want_bs = 0, ratio = 0, reserve_pct = 5, jmib = 0;
    const char *label = "", *uuid_s = 0;
    int whole = 0, journal = 1, c;
    while ((c = getopt(argc, argv, "b:i:J:L:U:m:O:IFqh")) != -1) {
        switch (c) {
        case 'b': want_bs = (uint32_t)atoi(optarg); break;
        case 'i': ratio = (uint32_t)atoi(optarg); break;
        case 'J':
            if (strncmp(optarg, "size=", 5)) { usage(); return 2; }
            jmib = (uint32_t)atoi(optarg + 5);
            break;
        case 'L': label = optarg; break;
        case 'U': uuid_s = optarg; break;
        case 'm': reserve_pct = (uint32_t)atoi(optarg); break;
        case 'O':
            if (!strcmp(optarg, "^has_journal")) journal = 0;
            else { fprintf(stderr, "mkfs.ext4: -O: only ^has_journal is known\n"); return 2; }
            break;
        case 'I': whole = 1; break;
        case 'F': case 'q': break;
        default: usage(); return 2;
        }
    }
    if (optind != argc - 1 || (want_bs && want_bs != 1024 && want_bs != 2048 && want_bs != 4096) || reserve_pct > 50) {
        usage();
        return 2;
    }
    if (strlen(label) > 16) {
        fprintf(stderr, "mkfs.ext4: a label has at most 16 characters\n");
        return 2;
    }
    uint8_t uuid[16];
    if (uuid_s ? parse_uuid(uuid_s, uuid) < 0 : getrandom(uuid, 16, 0) != 16) {
        fprintf(stderr, "mkfs.ext4: %s\n", uuid_s ? "a UUID is 32 hex digits" : "no random numbers");
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
        fprintf(stderr, "mkfs.ext4: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    if (info.flags & RX_BLOCK_FLAG_READONLY) {
        fprintf(stderr, "mkfs.ext4: %s: read-only device\n", dev);
        return 1;
    }
    if (!(info.flags & RX_BLOCK_FLAG_PARTITION) && (info.flags & (RX_BLOCK_FLAG_GPT | RX_BLOCK_FLAG_MBR)) && !whole) {
        fprintf(stderr, "mkfs.ext4: %s has a partition table: name a partition (or -I for the whole disk)\n", dev);
        return 1;
    }
    if (mounted(dev)) {
        fprintf(stderr, "mkfs.ext4: %s is mounted (umount it first)\n", dev);
        return 1;
    }
    dev_bs = info.block_size;
    uint64_t bytes = info.block_count * dev_bs;
    bs = want_bs ? want_bs : bytes < (4ull << 20) ? 1024 : 4096;
    if (bs < dev_bs) bs = dev_bs;
    if (bs > 4096) {
        fprintf(stderr, "mkfs.ext4: %s: sectors of %u bytes are too large\n", dev, dev_bs);
        return 1;
    }
    spb = bs / dev_bs;
    if (!ratio) ratio = bytes < (512ull << 20) ? 4096 : 16384;
    if (ratio < bs) ratio = bs;
    uint64_t nb64 = bytes / bs;
    if (nb64 > 0xFFFFFFFFull) nb64 = 0xFFFFFFFFull;
    nblocks = (uint32_t)nb64;
    uint32_t first_data = bs == 1024 ? 1 : 0, bpg = bs * 8, desc = 64, isz = 256, flex = 16;
    if (nblocks < 512) {
        fprintf(stderr, "mkfs.ext4: %s is too small\n", dev);
        return 1;
    }
    uint32_t ngroups = (nblocks - first_data + bpg - 1) / bpg;
    uint32_t ipb = bs / isz;
    uint64_t want_inodes = bytes / ratio;
    uint32_t ipg = (uint32_t)((want_inodes + ngroups - 1) / ngroups);
    uint32_t unit = ipb > 8 ? ipb : 8;
    if (ipg < 16) ipg = 16;
    ipg = (ipg + unit - 1) / unit * unit;
    if (ipg > bs * 8) ipg = bs * 8;
    uint32_t itb = ipg / ipb;
    uint32_t gdt_blocks = (ngroups * desc + bs - 1) / bs;
    // A last group too small for its own superblock copy is dropped
    for (;;) {
        uint32_t last = nblocks - first_data - (ngroups - 1) * bpg;
        uint8_t fake[1024] = { 0 };
        put32(fake + 100, RO_SPARSE_SUPER);
        uint32_t overhead = x4_has_super(fake, ngroups - 1) ? 1 + gdt_blocks : 0;
        if (ngroups > 1 && last < overhead + 50) {
            nblocks -= last;
            ngroups--;
            gdt_blocks = (ngroups * desc + bs - 1) / bs;
            continue;
        }
        break;
    }
    uint32_t ninodes = ipg * ngroups;
    uint32_t jblocks = 0;
    if (journal) {
        jblocks = jmib ? (uint32_t)((uint64_t)jmib * 1048576 / bs) : journal_blocks(nblocks);
        if (jblocks > 131072) jblocks = 131072;
        if (jblocks && jblocks < 1024) jblocks = 1024;
        if (!jblocks || jblocks > nblocks / 2) {
            fprintf(stderr, "mkfs.ext4: %s: too small for a journal (-O ^has_journal makes one without)\n", dev);
            return 1;
        }
    }

    uint8_t s[1024] = { 0 };
    put32(s + 100, RO_SPARSE_SUPER);   // for x4_has_super while laying out
    used = calloc(nblocks / 8 + 1, 1);
    uint32_t *bbm = calloc(ngroups, 4), *ibm = calloc(ngroups, 4), *itab = calloc(ngroups, 4);
    uint8_t *gdt = calloc(gdt_blocks, bs), *buf = calloc(1, bs), *bm = calloc(1, bs);
    if (!used || !bbm || !ibm || !itab || !gdt || !buf || !bm) {
        fprintf(stderr, "mkfs.ext4: out of memory\n");
        return 1;
    }
    // Superblock copies and descriptor tables; block 0 with 1 KiB blocks
    if (first_data) use(0, 1);
    for (uint32_t g = 0; g < ngroups; g++)
        if (x4_has_super(s, g)) use(first_data + g * bpg, 1 + gdt_blocks);
    // Each flexible group: its groups' block bitmaps, inode bitmaps, inode tables
    for (uint32_t f = 0; f * flex < ngroups; f++) {
        uint32_t g0 = f * flex, g1 = g0 + flex < ngroups ? g0 + flex : ngroups;
        uint32_t at = first_data + g0 * bpg;
        for (uint32_t g = g0; g < g1; g++) if (!(bbm[g] = take(&at, 1))) goto full;
        for (uint32_t g = g0; g < g1; g++) if (!(ibm[g] = take(&at, 1))) goto full;
        for (uint32_t g = g0; g < g1; g++) if (!(itab[g] = take(&at, itb))) goto full;
    }
    // Root, lost+found (16 KiB, so fsck can use it), the journal
    uint32_t at = first_data, lf_n = 16384 / bs;
    uint32_t root_blk = take(&at, 1), lf_blk = take(&at, lf_n);
    uint32_t mid = first_data + (ngroups / 2) * bpg, jstart = 0;
    if (journal) {
        uint32_t from = ngroups > 2 ? mid : at;
        if (!(jstart = take(&from, jblocks))) { from = at; jstart = take(&from, jblocks); }
    }
    if (!root_blk || !lf_blk || (journal && !jstart)) goto full;

    time_t now = time(0);
    printf("mkfs.ext4: %s: %u blocks of %u bytes, %u inodes, %u groups%s\n", dev, nblocks, bs, ninodes, ngroups,
           journal ? "" : ", no journal");
    fflush(stdout);

    // The superblock (its counts are filled in below)
    put32(s, ninodes);
    put32(s + 4, nblocks);
    put32(s + 8, (uint32_t)((uint64_t)nblocks * reserve_pct / 100));
    put32(s + 20, first_data);
    put32(s + 24, bs == 1024 ? 0 : bs == 2048 ? 1 : 2);
    put32(s + 28, bs == 1024 ? 0 : bs == 2048 ? 1 : 2);
    put32(s + 32, bpg);
    put32(s + 36, bpg);
    put32(s + 40, ipg);
    put32(s + 48, (uint32_t)now);
    put16(s + 54, 0xFFFF);
    put16(s + 56, 0xEF53);
    put16(s + 58, 1);
    put16(s + 60, 1);
    put32(s + 64, (uint32_t)now);
    put32(s + 76, 1);
    put32(s + 84, 11);
    put16(s + 88, isz);
    put32(s + 92, (journal ? COMPAT_HAS_JOURNAL : 0) | COMPAT_EXT_ATTR | COMPAT_DIR_INDEX);
    put32(s + 96, INCOMPAT_FILETYPE | INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_FLEX_BG);
    put32(s + 100, RO_SPARSE_SUPER | RO_LARGE_FILE | RO_HUGE_FILE | RO_DIR_NLINK | RO_EXTRA_ISIZE | RO_METADATA_CSUM);
    memcpy(s + 104, uuid, 16);
    memcpy(s + 120, label, strlen(label));
    uint32_t seed[4];
    if (getrandom(seed, sizeof seed, 0) != sizeof seed) memcpy(seed, uuid, 16);
    memcpy(s + 236, seed, 16);   // directory hash seed
    s[252] = 1;                  // half_md4
    put16(s + 254, desc);
    put32(s + 264, (uint32_t)now);
    put16(s + 348, 32);
    put16(s + 350, 32);
    put32(s + 352, 1);           // signed directory hash
    s[372] = 4;                  // 16 groups per flexible group
    s[373] = 1;                  // checksums: crc32c
    if (journal) {
        put32(s + 224, 8);
        s[253] = 1;              // the journal inode's block map is copied below
    }
    struct x4 X;
    x4_setup(&X, s);

    // Inodes: 2 (root), 8 (journal), 11 (lost+found)
    uint8_t *it = calloc(1, (size_t)itb * bs);
    if (!it) return 1;
    uint8_t *rp = it + 1 * isz, *jp = it + 7 * isz, *lp = it + 10 * isz;
    struct { uint8_t *p; uint32_t mode, size, start, n, links; } mk[3] = {
        { rp, 040755, bs, root_blk, 1, 3 },
        { lp, 040700, lf_n * bs, lf_blk, lf_n, 2 },
        { jp, 0100600, jblocks * bs, jstart, jblocks, 1 },
    };
    for (int k = 0; k < (journal ? 3 : 2); k++) {
        uint8_t *p = mk[k].p;
        put16(p, mk[k].mode);
        put32(p + 4, mk[k].size);
        put32(p + 8, (uint32_t)now);
        put32(p + 12, (uint32_t)now);
        put32(p + 16, (uint32_t)now);
        put16(p + 26, mk[k].links);
        put32(p + 28, mk[k].n * (bs / 512));
        put32(p + 32, EXTENTS_FL);
        // The extent root: up to 4 extents of 32768 blocks
        put16(p + 40, XMAGIC);
        put16(p + 44, 4);
        uint32_t left = mk[k].n, lb = 0, e = 0;
        while (left) {
            uint32_t len = left > 32768 ? 32768 : left;
            uint8_t *x = p + 52 + 12 * e++;
            put32(x, lb);
            put16(x + 4, len);
            put32(x + 8, mk[k].start + lb);
            lb += len;
            left -= len;
        }
        put16(p + 42, e);
        put16(p + 128, 32);
        put32(p + 144, (uint32_t)now);
    }
    if (journal) {
        // A copy of the journal's block map, for e2fsck
        for (int k = 0; k < 15; k++) put32(s + 0x10C + 4 * k, le32(jp + 40 + 4 * k));
        put32(s + 0x10C + 60, 0);
        put32(s + 0x10C + 64, jblocks * bs);
    }
    for (uint32_t i = 0; i < itb * ipb; i++) {
        uint8_t *p = it + (size_t)i * isz;
        if (le16(p)) x4_inode_csum_set(&X, i + 1, p);
    }

    // Directory blocks
    uint32_t root_seed = x4_inode_seed(&X, 2, 0), lf_seed = x4_inode_seed(&X, 11, 0);
    memset(buf, 0, bs);
    put32(buf, 2); put16(buf + 4, 12); buf[6] = 1; buf[7] = 2; buf[8] = '.';
    put32(buf + 12, 2); put16(buf + 16, 12); buf[18] = 2; buf[19] = 2; buf[20] = '.'; buf[21] = '.';
    put32(buf + 24, 11); put16(buf + 28, bs - 12 - 24); buf[30] = 10; buf[31] = 2; memcpy(buf + 32, "lost+found", 10);
    x4_dir_init_tail(bs, buf);
    put32(buf + bs - 4, x4_dir_csum(root_seed, bs, buf));
    if (wr(root_blk, 1, buf) < 0) return 1;
    memset(buf, 0, bs);
    put32(buf, 11); put16(buf + 4, 12); buf[6] = 1; buf[7] = 2; buf[8] = '.';
    put32(buf + 12, 2); put16(buf + 16, bs - 12 - 12); buf[18] = 2; buf[19] = 2; buf[20] = '.'; buf[21] = '.';
    x4_dir_init_tail(bs, buf);
    put32(buf + bs - 4, x4_dir_csum(lf_seed, bs, buf));
    if (wr(lf_blk, 1, buf) < 0) return 1;
    memset(buf, 0, bs);
    put16(buf + 4, bs - 12);
    x4_dir_init_tail(bs, buf);
    put32(buf + bs - 4, x4_dir_csum(lf_seed, bs, buf));
    for (uint32_t k = 1; k < lf_n; k++) if (wr(lf_blk + k, 1, buf) < 0) return 1;

    // The journal: its superblock (empty, checksummed, 64-bit tags)
    if (journal) {
        uint32_t jseq;
        if (getrandom(&jseq, 4, 0) != 4) jseq = (uint32_t)now;
        jseq = (jseq & 0x7FFFFFFF) | 1;
        memset(buf, 0, bs);
        putbe32(buf, JBD_MAGIC);
        putbe32(buf + 4, JBD_SB_V2);
        putbe32(buf + 0xC, bs);
        putbe32(buf + 0x10, jblocks);
        putbe32(buf + 0x14, 1);
        putbe32(buf + 0x18, jseq);
        putbe32(buf + 0x28, JBD_INCOMPAT_64BIT | JBD_INCOMPAT_CSUM_V3);
        memcpy(buf + 0x30, uuid, 16);
        putbe32(buf + 0x40, 1);   // users
        buf[0x50] = 4;            // crc32c
        memcpy(buf + 0x100, uuid, 16);
        putbe32(buf + 0xFC, jbd_sb_csum(buf));
        if (wr(jstart, 1, buf) < 0) return 1;
        // Old journal contents cannot be taken for transactions: they would
        // need this sequence number. The next block is cleared anyway.
        memset(buf, 0, bs);
        if (wr(jstart + 1, 1, buf) < 0) return 1;
    }

    // Groups: bitmaps, descriptors; "uninit" where nothing but the group's
    // own metadata is in it
    uint32_t total_free = 0;
    for (uint32_t g = 0; g < ngroups; g++) {
        uint32_t start = first_data + g * bpg, n = g == ngroups - 1 ? nblocks - start : bpg, gfree = 0;
        memset(bm, 0, bs);
        for (uint32_t i = 0; i < n; i++) {
            if (is_used(start + i)) bm[i / 8] |= (uint8_t)(1u << (i % 8));
            else gfree++;
        }
        for (uint32_t i = n; i < bs * 8; i++) bm[i / 8] |= (uint8_t)(1u << (i % 8));
        // What a BLOCK_UNINIT group's bitmap is taken to be
        uint8_t *expect = buf;
        memset(expect, 0, bs);
        if (x4_has_super(s, g)) for (uint32_t i = 0; i < 1 + gdt_blocks; i++) expect[i / 8] |= (uint8_t)(1u << (i % 8));
        uint32_t own[3] = { bbm[g], ibm[g], itab[g] };
        for (int k = 0; k < 3; k++)
            for (uint32_t b = own[k]; b < own[k] + (k == 2 ? itb : 1); b++)
                if (b >= start && b - start < n) expect[(b - start) / 8] |= (uint8_t)(1u << ((b - start) % 8));
        for (uint32_t i = n; i < bs * 8; i++) expect[i / 8] |= (uint8_t)(1u << (i % 8));
        int block_uninit = g && g != ngroups - 1 && !memcmp(expect, bm, bs);
        uint8_t *d = gdt + (size_t)g * desc;
        put32(d, bbm[g]);
        put32(d + 4, ibm[g]);
        put32(d + 8, itab[g]);
        put16(d + 12, gfree);
        put16(d + 14, g == 0 ? ipg - 11 : ipg);
        put16(d + 16, g == 0 ? 2 : 0);
        put16(d + 18, (g ? BG_INODE_UNINIT : BG_INODE_ZEROED) | (block_uninit ? BG_BLOCK_UNINIT : 0));
        uint32_t bc = x4_bitmap_csum(&X, bm, bpg);
        put16(d + 0x18, bc);
        put16(d + 0x38, bc >> 16);
        if (wr(bbm[g], 1, bm) < 0) return 1;
        memset(bm, 0, bs);
        if (g == 0) for (uint32_t i = 0; i < 11; i++) bm[i / 8] |= (uint8_t)(1u << (i % 8));
        for (uint32_t i = ipg; i < bs * 8; i++) bm[i / 8] |= (uint8_t)(1u << (i % 8));
        uint32_t ic = x4_bitmap_csum(&X, bm, ipg);
        put16(d + 0x1A, ic);
        put16(d + 0x3A, ic >> 16);
        if (wr(ibm[g], 1, bm) < 0) return 1;
        put16(d + 0x1C, g == 0 ? ipg - 11 : ipg);   // never-used inodes at the table's end
        put16(d + 0x1E, x4_gd_csum(&X, g, d));
        total_free += gfree;
    }
    // The first inode table (the others are left as they are: never used)
    if (wr(itab[0], itb, it) < 0) return 1;

    put32(s + 12, total_free);
    put32(s + 16, ninodes - 11);
    // Clear the start (boot sectors, old signatures), then the copies
    if (zero(0, first_data + 1) < 0) return 1;
    for (uint32_t g = 0; g < ngroups; g++) {
        if (!x4_has_super(s, g)) continue;
        uint32_t start = first_data + g * bpg;
        put16(s + 90, g);
        put32(s + 1020, x4_sb_csum(s));
        if (g == 0) {
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
        fprintf(stderr, "mkfs.ext4: %s: flush: %s\n", dev, strerror(-r));
        return 1;
    }
    printf("mkfs.ext4: UUID %02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x%s\n", uuid[0], uuid[1],
           uuid[2], uuid[3], uuid[4], uuid[5], uuid[6], uuid[7], uuid[8], uuid[9], uuid[10], uuid[11], uuid[12], uuid[13],
           uuid[14], uuid[15], journal ? "" : "");
    if (journal) printf("mkfs.ext4: journal of %u blocks\n", jblocks);
    return 0;
full:
    fprintf(stderr, "mkfs.ext4: %s: no room for the groups' tables (try -i with a larger number)\n", dev);
    return 1;
}
