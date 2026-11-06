// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fsck.ext2: check an ext2 filesystem (step 15).
//
//   fsck.ext2 [-a] [-v] DEV
//
// The passes of e2fsck, without repairs: (1) every inode in use: its
// type, its blocks (in range, used once, counted right in i_blocks, none
// past the size); (2) every directory: entries, ".", "..", file types;
// (3) every directory reachable from the root; (4) link counts, inodes
// in use that no directory names; (5) the block and inode bitmaps and the
// free counts. It changes nothing, except with -a: then wrong free counts
// (superblock and groups) are corrected. Run it on a filesystem that is
// not mounted; e2fsck -f on Linux does the repairs.
// Exit status: 0 no errors, 1 errors corrected, 4 errors left.

#include <errno.h>
#include <rustux/device.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static rx_handle_t ch;
static uint32_t dev_bs, bs, spb, nblocks, ninodes, first_data, bpg, ipg, ngroups, isz, first_ino;
static uint32_t feat_incompat;
static uint8_t sb[1024], *gdt, *used, *ibits;   // used: blocks seen; ibits: on-disk inode bitmap
static uint16_t *refs, *subdirs;
static uint8_t *is_dir, *reached, *inuse;
static uint32_t *parent_of;
static int errors, verbose, files, dirs, max_errors = 50;
static const char *dev;

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
static uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static void put16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

static void err(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void err(const char *fmt, ...) {
    errors++;
    if (errors > max_errors) {
        if (errors == max_errors + 1) printf("(more errors not shown)\n");
        return;
    }
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
    printf("\n");
}

static int rd(uint32_t block, uint32_t count, void *buf) {
    int r = rx_block_read(ch, dev_bs, (uint64_t)block * spb, count * spb, buf);
    if (r < 0) err("read error at block %u: %s", block, strerror(-r));
    return r;
}

static int bit(const uint8_t *m, uint32_t i) { return (m[i / 8] >> (i % 8)) & 1; }
static void setbit(uint8_t *m, uint32_t i) { m[i / 8] |= (uint8_t)(1u << (i % 8)); }

static int has_super(uint32_t g) {
    if (!(le32(sb + 100) & 1)) return 1;   // no sparse_super: every group
    if (g <= 1) return 1;
    for (uint32_t b = 3; b <= 7; b += 2) {
        uint64_t p = b;
        while (p < g) p *= b;
        if (p == g) return 1;
    }
    return 0;
}

// Mark block b as used by `what`; 0 if it is out of range or used twice
static int claim(uint32_t b, uint32_t ino, const char *what) {
    if (b < first_data || b >= nblocks) {
        err("inode %u: %s block %u is out of range", ino, what, b);
        return 0;
    }
    if (bit(used, b)) {
        err("inode %u: %s block %u is used twice", ino, what, b);
        return 0;
    }
    setbit(used, b);
    return 1;
}

static uint8_t *inode_buf;
static uint32_t inode_buf_group = UINT32_MAX;

static uint8_t *get_inode(uint32_t ino) {
    uint32_t g = (ino - 1) / ipg, i = (ino - 1) % ipg;
    if (g != inode_buf_group) {
        uint32_t table = le32(gdt + g * 32 + 8);
        uint32_t nb = (ipg * isz + bs - 1) / bs;
        if (rd(table, nb, inode_buf) < 0) return 0;
        inode_buf_group = g;
    }
    return inode_buf + (size_t)i * isz;
}

// Count (and claim) the blocks under a pointer block; *last gets the
// highest logical block seen
static uint64_t walk_ind(uint32_t ino, uint32_t b, int depth, uint64_t base, uint64_t *last, int *bad) {
    uint64_t count = 0;
    if (!claim(b, ino, "indirect")) { *bad = 1; return 0; }
    count++;
    uint8_t *buf = malloc(bs);
    if (!buf || rd(b, 1, buf) < 0) { free(buf); *bad = 1; return count; }
    uint32_t per = bs / 4;
    uint64_t span = 1;
    for (int k = 1; k < depth; k++) span *= per;
    for (uint32_t i = 0; i < per; i++) {
        uint32_t p = le32(buf + 4 * i);
        if (!p) continue;
        if (depth == 1) {
            if (claim(p, ino, "data")) count++;
            else *bad = 1;
            if (base + i > *last || *last == UINT64_MAX) *last = base + i;
        } else {
            count += walk_ind(ino, p, depth - 1, base + i * span, last, bad);
        }
    }
    free(buf);
    return count;
}

static void check_inode(uint32_t ino, uint8_t *p) {
    uint32_t mode = le16(p), type = mode & 0170000;
    uint64_t size = le32(p + 4) | (type == 0100000 ? (uint64_t)le32(p + 108) << 32 : 0);
    uint64_t iblocks = le32(p + 28) | (uint64_t)le16(p + 116) << 32;
    if (le32(p + 32) & 0x80000) { err("inode %u uses extents (ext4: step 16)", ino); return; }
    int reserved = ino < first_ino && ino != 2;
    if (!reserved && type != 0100000 && type != 0040000 && type != 0120000 && type != 0020000 && type != 0060000 &&
        type != 0010000 && type != 0140000) {
        err("inode %u has a bad type %06o", ino, mode);
        return;
    }
    if (type == 0040000) { is_dir[ino] = 1; dirs++; }
    else if (!reserved) files++;
    // Fast symbolic links and device files have no blocks
    int fast = (type == 0120000 && (iblocks == 0 || (le32(p + 104) && iblocks == bs / 512))) ||
               type == 0020000 || type == 0060000 || type == 0010000 || type == 0140000;
    uint64_t count = 0, last = UINT64_MAX;
    int bad = 0;
    if (!fast) {
        for (int i = 0; i < 12; i++) {
            uint32_t b = le32(p + 40 + 4 * i);
            if (!b) continue;
            if (claim(b, ino, "data")) count++;
            else bad = 1;
            last = (uint64_t)i;
        }
        uint32_t per = bs / 4;
        uint64_t base = 12, span = per;
        for (int d = 1; d <= 3; d++) {
            uint32_t b = le32(p + 40 + 4 * (11 + d));
            if (b) count += walk_ind(ino, b, d, base, &last, &bad);
            base += span;
            span *= per;
        }
    }
    if (le32(p + 104)) {
        uint32_t ea = le32(p + 104);
        // An extended attribute block may be shared: count it, claim it once
        if (ea >= first_data && ea < nblocks) { if (!bit(used, ea)) setbit(used, ea); count++; }
        else err("inode %u: extended attribute block %u is out of range", ino, ea);
    }
    if (reserved) return;
    if (!bad && count * (bs / 512) != iblocks)
        err("inode %u: i_blocks is %llu, should be %llu", ino, (unsigned long long)iblocks, (unsigned long long)(count * (bs / 512)));
    if (!bad && !fast && last != UINT64_MAX) {
        // No block past the size (files may have holes before it)
        if (type == 0040000 ? size < (last + 1) * bs : last >= (size + bs - 1) / bs)
            err("inode %u: size %llu, but it has a block at %llu", ino, (unsigned long long)size, (unsigned long long)(last * bs));
    }
    if (type == 0040000 && size % bs) err("directory inode %u: size %llu is not whole blocks", ino, (unsigned long long)size);
    if (type == 0120000 && fast && size >= 60) err("symbolic link inode %u: fast link of %llu bytes", ino, (unsigned long long)size);
}

// The logical block lb of an inode (0: none)
static uint32_t bmap(uint8_t *p, uint64_t lb) {
    uint32_t per = bs / 4;
    uint8_t buf[4096];
    if (lb < 12) return le32(p + 40 + 4 * lb);
    lb -= 12;
    uint32_t idx[3];
    int depth;
    if (lb < per) { depth = 1; idx[0] = (uint32_t)lb; }
    else if ((lb -= per) < (uint64_t)per * per) { depth = 2; idx[0] = (uint32_t)(lb / per); idx[1] = (uint32_t)(lb % per); }
    else { lb -= (uint64_t)per * per; depth = 3; idx[0] = (uint32_t)(lb / per / per); idx[1] = (uint32_t)(lb / per % per); idx[2] = (uint32_t)(lb % per); }
    uint32_t b = le32(p + 40 + 4 * (11 + depth));
    for (int i = 0; i < depth && b; i++) {
        if (b >= nblocks || rx_block_read(ch, dev_bs, (uint64_t)b * spb, spb, buf) < 0) return 0;
        b = le32(buf + 4 * idx[i]);
    }
    return b;
}

static const char *type_name(uint32_t t) { return t == 1 ? "file" : t == 2 ? "directory" : t == 7 ? "link" : "special file"; }

static void check_dir(uint32_t dino) {
    uint8_t *ip = get_inode(dino);
    if (!ip) return;
    uint8_t ino_copy[512];
    memcpy(ino_copy, ip, isz);
    uint64_t size = le32(ino_copy + 4);
    uint8_t *buf = malloc(bs);
    if (!buf) return;
    int saw_dot = 0, saw_dotdot = 0;
    for (uint64_t lb = 0; lb * bs < size; lb++) {
        uint32_t b = bmap(ino_copy, lb);
        if (!b) { err("directory %u: block %llu is a hole", dino, (unsigned long long)lb); continue; }
        if (rd(b, 1, buf) < 0) continue;
        for (uint32_t off = 0; off < bs;) {
            uint8_t *e = buf + off;
            uint32_t rec = le16(e + 4), nl = (feat_incompat & 2) ? e[6] : le16(e + 6), ino = le32(e);
            if (rec < 8 || rec % 4 || off + rec > bs || (ino && 8 + nl > rec)) {
                err("directory %u: a damaged entry in block %llu at %u", dino, (unsigned long long)lb, off);
                break;
            }
            off += rec;
            if (!ino) continue;
            char name[256];
            memcpy(name, e + 8, nl);
            name[nl] = 0;
            int first = lb == 0 && off - rec == 0, second = lb == 0 && off - rec == 12;
            if (ino > ninodes || !inuse[ino]) {
                err("directory %u: \"%s\" names inode %u, which is %s", dino, name, ino, ino > ninodes ? "out of range" : "not in use");
                continue;
            }
            if (!strcmp(name, ".")) {
                if (!first) err("directory %u: \".\" is not the first entry", dino);
                if (ino != dino) err("directory %u: \".\" names inode %u", dino, ino);
                saw_dot = 1;
                continue;
            }
            if (!strcmp(name, "..")) {
                if (!second) err("directory %u: \"..\" is not the second entry", dino);
                parent_of[dino] = ino;
                saw_dotdot = 1;
                continue;
            }
            if (memchr(name, '/', nl)) err("directory %u: \"%s\" contains a slash", dino, name);
            if (feat_incompat & 2) {
                uint8_t *tp = get_inode(ino);
                uint32_t t = tp ? le16(tp) & 0170000 : 0;
                uint32_t want = t == 0100000 ? 1 : t == 0040000 ? 2 : t == 0020000 ? 3 : t == 0060000 ? 4 : t == 0010000 ? 5 : t == 0140000 ? 6 : 7;
                if (e[7] != want) err("directory %u: \"%s\" is listed as a %s, but is a %s", dino, name, type_name(e[7]), type_name(want));
            }
            refs[ino]++;
            if (is_dir[ino]) {
                if (reached[ino]) err("directory %u: \"%s\": directory %u has a second name", dino, name, ino);
                else {
                    reached[ino] = 1;
                    subdirs[dino]++;
                    if (verbose) printf("  directory %u -> %u (%s)\n", dino, ino, name);
                    check_dir(ino);
                    ip = 0;
                }
            }
        }
    }
    if (!saw_dot) err("directory %u has no \".\"", dino);
    if (!saw_dotdot) err("directory %u has no \"..\"", dino);
    free(buf);
}

int main(int argc, char **argv) {
    int fix = 0, c;
    while ((c = getopt(argc, argv, "anvyf")) != -1) {
        if (c == 'a' || c == 'y') fix = 1;
        else if (c == 'v') verbose = 1;
        else if (c != 'n' && c != 'f') {
            fprintf(stderr, "usage: fsck.ext2 [-a] [-v] DEV\n");
            return 16;
        }
    }
    if (optind != argc - 1) {
        fprintf(stderr, "usage: fsck.ext2 [-a] [-v] DEV\n");
        return 16;
    }
    dev = argv[optind];
    if (!strncmp(dev, "/dev/", 5)) dev += 5;
    rx_block_info_t info;
    int r = rx_block_open(dev, &ch);
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "fsck.ext2: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 8;
    }
    dev_bs = info.block_size;
    uint8_t *tmp = malloc(4096);
    uint64_t first = 1024 / dev_bs, cnt = (2048 + dev_bs - 1) / dev_bs - first;
    if (!tmp || rx_block_read(ch, dev_bs, first, (uint32_t)cnt, tmp) < 0) {
        fprintf(stderr, "fsck.ext2: %s: cannot read the superblock\n", dev);
        return 8;
    }
    memcpy(sb, tmp + 1024 % dev_bs, 1024);
    if (le16(sb + 56) != 0xEF53) {
        printf("%s: not an ext2 filesystem (no superblock)\n", dev);
        return 8;
    }
    bs = 1024u << le32(sb + 24);
    if (le32(sb + 24) > 2 || bs < dev_bs) {
        printf("%s: block size %u cannot be read here\n", dev, bs);
        return 8;
    }
    spb = bs / dev_bs;
    nblocks = le32(sb + 4);
    ninodes = le32(sb);
    first_data = le32(sb + 20);
    bpg = le32(sb + 32);
    ipg = le32(sb + 40);
    isz = le32(sb + 76) ? le16(sb + 88) : 128;
    first_ino = le32(sb + 76) ? le32(sb + 84) : 11;
    feat_incompat = le32(sb + 76) ? le32(sb + 96) : 0;
    if (!bpg || !ipg || nblocks <= first_data || isz < 128 || isz > 512) {
        printf("%s: the superblock is damaged\n", dev);
        return 8;
    }
    ngroups = (nblocks - first_data + bpg - 1) / bpg;
    if ((uint64_t)ngroups * ipg != ninodes) err("superblock: %u inodes, but %u groups of %u", ninodes, ngroups, ipg);
    if (feat_incompat & ~2u) printf("%s: has features fsck.ext2 does not check (incompat %#x)\n", dev, feat_incompat & ~2u);
    if (!(le16(sb + 58) & 1)) printf("%s: was not cleanly unmounted\n", dev);
    printf("%s: ext2, %u blocks of %u bytes, %u inodes, %u groups\n", dev, nblocks, bs, ninodes, ngroups);
    uint32_t gdt_blocks = (ngroups * 32 + bs - 1) / bs;
    gdt = malloc((size_t)gdt_blocks * bs);
    used = calloc(nblocks / 8 + 1, 1);
    ibits = calloc(ninodes / 8 + 1, 1);
    refs = calloc(ninodes + 1, 2);
    subdirs = calloc(ninodes + 1, 2);
    is_dir = calloc(ninodes + 1, 1);
    reached = calloc(ninodes + 1, 1);
    inuse = calloc(ninodes + 1, 1);
    parent_of = calloc(ninodes + 1, 4);
    inode_buf = malloc((size_t)((ipg * isz + bs - 1) / bs) * bs);
    uint8_t *bm = malloc(bs);
    if (!gdt || !used || !ibits || !refs || !subdirs || !is_dir || !reached || !inuse || !parent_of || !inode_buf || !bm) {
        fprintf(stderr, "fsck.ext2: out of memory\n");
        return 8;
    }
    if (rd(first_data + 1, gdt_blocks, gdt) < 0) return 8;
    // Metadata: superblock copies, descriptor tables (and the blocks kept
    // for growing them), bitmaps, inode tables
    uint32_t resv = le16(sb + 206);
    for (uint32_t g = 0; g < ngroups; g++) {
        uint32_t start = first_data + g * bpg;
        if (has_super(g)) for (uint32_t k = 0; k < 1 + gdt_blocks + resv; k++) setbit(used, start + k);
        uint8_t *d = gdt + g * 32;
        uint32_t b = le32(d), i = le32(d + 4), t = le32(d + 8), tb = (ipg * isz + bs - 1) / bs;
        if (b < first_data || b >= nblocks || i < first_data || i >= nblocks || t < first_data || t + tb > nblocks) {
            err("group %u: its bitmaps or inode table are out of range", g);
            continue;
        }
        setbit(used, b);
        setbit(used, i);
        for (uint32_t k = 0; k < tb; k++) setbit(used, t + k);
    }
    if (first_data) setbit(used, 0);   // block 0 (1 KiB blocks): boot sector, outside the groups

    // Pass 1: inodes
    for (uint32_t g = 0; g < ngroups; g++) {
        if (rd(le32(gdt + g * 32 + 4), 1, bm) < 0) continue;
        for (uint32_t i = 0; i < ipg; i++) {
            uint32_t ino = g * ipg + i + 1;
            if (bit(bm, i)) setbit(ibits, ino - 1);
            uint8_t *p = get_inode(ino);
            if (!p) continue;
            int live = le16(p + 26) && !le32(p + 20) && le16(p);
            if (ino < first_ino && ino != 2) {
                // Reserved inodes: their blocks count. The resize inode's
                // blocks are the reserved descriptor blocks (counted above)
                // but for its own pointer block
                if (ino == 7 && le32(p + 40 + 4 * 13)) setbit(used, le32(p + 40 + 4 * 13));
                else if (le16(p)) check_inode(ino, p);
                inuse[ino] = bit(bm, i);
                continue;
            }
            if (bit(bm, i) && !live) {
                if (le16(p)) err("inode %u is marked in use but deleted", ino);
                continue;
            }
            if (!bit(bm, i)) {
                if (live) err("inode %u is in use but not marked in the bitmap", ino);
                continue;
            }
            inuse[ino] = 1;
            check_inode(ino, p);
        }
    }
    // Passes 2 and 3: directories from the root
    if (!inuse[2] || !is_dir[2]) err("the root directory (inode 2) is missing");
    else {
        reached[2] = 1;
        check_dir(2);
        if (parent_of[2] != 2) err("the root directory's \"..\" names inode %u", parent_of[2]);
    }
    // Pass 4: links
    for (uint32_t ino = first_ino; ino <= ninodes; ino++) {
        if (!inuse[ino]) continue;
        uint8_t *p = get_inode(ino);
        if (!p) continue;
        uint32_t links = le16(p + 26);
        if (is_dir[ino]) {
            if (!reached[ino]) { err("directory inode %u is not reachable from the root", ino); continue; }
            uint32_t want = 2 + subdirs[ino];
            if (links != want && !(links == 1 && want >= 65000)) err("directory inode %u: %u links, should be %u", ino, links, want);
        } else {
            if (!refs[ino]) { err("inode %u is in use but no directory names it", ino); continue; }
            if (links != refs[ino]) err("inode %u: %u links, should be %u", ino, links, refs[ino]);
        }
    }
    {
        uint8_t *p = get_inode(2);
        if (p && inuse[2] && le16(p + 26) != 2 + subdirs[2])
            err("the root directory: %u links, should be %u", le16(p + 26), 2 + subdirs[2]);
    }
    // Pass 5: bitmaps and counts
    uint32_t free_total = 0, ifree_total = 0, bad_blocks = 0, bad_inodes = 0;
    int fixed = 0, gdt_changed = 0;
    for (uint32_t g = 0; g < ngroups; g++) {
        uint8_t *d = gdt + g * 32;
        uint32_t start = first_data + g * bpg, in_group = g == ngroups - 1 ? nblocks - start : bpg;
        if (rd(le32(d), 1, bm) < 0) continue;
        uint32_t gfree = 0, gdirs = 0, gifree = 0;
        for (uint32_t i = 0; i < in_group; i++) {
            int disk = bit(bm, i), seen = bit(used, start + i);
            if (disk != seen) bad_blocks++;
            if (!disk) gfree++;
        }
        for (uint32_t i = in_group; i < bs * 8; i++) if (!bit(bm, i)) { err("group %u: the block bitmap's padding is not set", g); break; }
        for (uint32_t i = 0; i < ipg; i++) {
            uint32_t ino = g * ipg + i + 1;
            if (!bit(ibits, ino - 1)) gifree++;
            else if (is_dir[ino] && inuse[ino]) gdirs++;
        }
        if (le16(d + 12) != gfree || le16(d + 14) != gifree || le16(d + 16) != gdirs) {
            printf("group %u: counts say %u free blocks, %u free inodes, %u directories; there are %u, %u, %u\n", g,
                   le16(d + 12), le16(d + 14), le16(d + 16), gfree, gifree, gdirs);
            if (fix) { put16(d + 12, gfree); put16(d + 14, gifree); put16(d + 16, gdirs); gdt_changed = 1; }
            else errors++;
        }
        free_total += gfree;
        ifree_total += gifree;
    }
    for (uint32_t ino = 1; ino <= ninodes; ino++) {
        int marked = bit(ibits, ino - 1);
        if (ino >= first_ino && marked != inuse[ino]) bad_inodes++;
    }
    if (bad_blocks) err("block bitmap: %u blocks are marked differently than they are used", bad_blocks);
    if (bad_inodes) err("inode bitmap: %u inodes are marked differently than they are used", bad_inodes);
    if (le32(sb + 12) != free_total || le32(sb + 16) != ifree_total) {
        printf("superblock: %u free blocks, %u free inodes; there are %u, %u\n", le32(sb + 12), le32(sb + 16), free_total, ifree_total);
        if (fix) {
            put32(sb + 12, free_total);
            put32(sb + 16, ifree_total);
            uint64_t f0 = 1024 / dev_bs, n0 = (2048 + dev_bs - 1) / dev_bs - f0;
            if (rx_block_read(ch, dev_bs, f0, (uint32_t)n0, tmp) == 0) {
                memcpy(tmp + 1024 % dev_bs, sb, 1024);
                if (rx_block_write(ch, dev_bs, f0, (uint32_t)n0, tmp) == 0) { printf("  corrected\n"); fixed = 1; }
            }
        } else {
            errors++;
        }
    }
    if (gdt_changed && rx_block_write(ch, dev_bs, (uint64_t)(first_data + 1) * spb, gdt_blocks * spb, gdt) == 0) {
        printf("  group counts corrected\n");
        fixed = 1;
    }
    if (fixed) rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
    printf("%s: %d files, %d directories, %u/%u blocks, %u/%u inodes in use%s\n", dev, files, dirs, nblocks - first_data - free_total,
           nblocks - first_data, ninodes - ifree_total, ninodes, errors ? "" : ", no errors");
    if (errors) {
        printf("%s: %d error%s (e2fsck -f on Linux can repair them)\n", dev, errors, errors == 1 ? "" : "s");
        return 4;
    }
    return fixed ? 1 : 0;
}
