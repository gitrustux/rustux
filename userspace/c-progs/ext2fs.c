// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ext2fs: the ext2 filesystem server (step 15). Serves an ext2 filesystem
// on a block device with fsd's file protocol (<rustux/fs.h>), read and
// write: block groups, block and inode bitmaps, inodes (128 or 256 bytes,
// nanosecond times in the larger ones), direct and indirect blocks,
// sparse files, directories (indexed ones are read as plain lists, and
// lose the index when changed, as Linux's ext2 driver does), symbolic and
// hard links, owners, permission bits.
//
//   ext2fs [-r] DEVICE      started by mount, with the server end of the
//                           root directory's connection as PA_USER0
//
// Filesystems with ext3/ext4 features this server does not keep up to
// date (a journal, checksums, ...) are served read-only; those it cannot
// read (extents, 64-bit, ...) are refused (step 16). While mounted for
// writing the superblock says "not clean"; SHUTDOWN marks it clean again,
// so e2fsck checks a filesystem that was not unmounted. Metadata reaches
// the disk before each reply, file data within half a second. Format:
// "The Second Extended File System" (Dave Poirier), and Linux's
// Documentation/filesystems/ext2.rst.

#include <errno.h>
#include <fcntl.h>
#include <rustux/device.h>
#include <rustux/fs.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <unistd.h>

#define MAX_CONNS 500
#define IOBUF (256u * 1024u)
#define CACHE 1024          // metadata blocks kept
#define PATHMAX 4096

// Features (superblock)
#define COMPAT_HAS_JOURNAL   0x0004
#define INCOMPAT_FILETYPE    0x0002
#define RO_SPARSE_SUPER      0x0001
#define RO_LARGE_FILE        0x0002
#define INDEX_FL             0x1000   // inode flag: hashed directory

#define S_IFMT_  0170000
#define S_IFDIR_ 0040000
#define S_IFREG_ 0100000
#define S_IFLNK_ 0120000
#define S_IFCHR_ 0020000

static const char *dev_name = "?";
static int read_only;

static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
static uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static void put16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }

// ---- the block device ----

static rx_handle_t blk;
static uint32_t dev_bs;
static uint64_t dev_blocks;
static uint8_t *iobuf;
static uint32_t vmoid;

static int dev_rw(int write, uint64_t block, uint32_t count) {
    rx_block_req_t q = { 0 };
    q.op = write ? RX_BLOCK_OP_WRITE_VMO : RX_BLOCK_OP_READ_VMO;
    q.block = block;
    q.count = count;
    q.vmoid = vmoid;
    q.vmo_offset = 0;
    int r = rx_block_call(blk, &q, 0, 0, 0, 0, 0, 0);
    return r == -EPIPE ? -EIO : r;   // the disk is gone
}

// ---- the filesystem ----

static uint8_t sb[1024];
static int sb_dirty;
static uint32_t bs, spb;          // block size; device blocks per block
static uint32_t nblocks, ninodes, first_data, bpg, ipg, ngroups, isz, first_ino, gdt_block;
static uint32_t feat_compat, feat_incompat, feat_ro;
static uint32_t fsid;

static uint64_t blk_dev(uint32_t b) { return (uint64_t)b * spb; }

// ---- block cache (metadata and directories) ----

struct cblk {
    uint32_t block;     // 0: empty (block 0 is never cached)
    uint64_t used;
    int dirty;
    uint8_t *data;
};

static struct cblk cache[CACHE];
static uint64_t cache_clock;
static int io_error;
static int write_pending;

static int write_cblk(struct cblk *c) {
    memcpy(iobuf, c->data, bs);
    int r = dev_rw(1, blk_dev(c->block), spb);
    if (r == 0) c->dirty = 0;
    else io_error = r;
    return r;
}

static struct cblk *get_block(uint32_t block) {
    struct cblk *victim = 0;
    if (block == 0 || block >= nblocks) {
        io_error = -EIO;
        return 0;
    }
    for (int i = 0; i < CACHE; i++) {
        if (cache[i].block == block) {
            cache[i].used = ++cache_clock;
            return &cache[i];
        }
        if (!victim || cache[i].used < victim->used) victim = &cache[i];
    }
    if (victim->dirty) write_cblk(victim);
    victim->block = 0;
    if (dev_rw(0, blk_dev(block), spb) < 0) {
        io_error = -EIO;
        return 0;
    }
    memcpy(victim->data, iobuf, bs);
    victim->block = block;
    victim->used = ++cache_clock;
    victim->dirty = 0;
    return victim;
}

// A fresh (zeroed) cached block, without reading it
static struct cblk *new_block(uint32_t block) {
    struct cblk *c = 0;
    for (int i = 0; i < CACHE && !c; i++) if (cache[i].block == block) c = &cache[i];
    if (!c) {
        for (int i = 0; i < CACHE; i++) if (!c || cache[i].used < c->used) c = &cache[i];
        if (c->dirty) write_cblk(c);
        c->block = block;
    }
    memset(c->data, 0, bs);
    c->used = ++cache_clock;
    c->dirty = 1;
    return c;
}

// A freed block is no longer metadata: drop it (dirty or not)
static void forget_block(uint32_t block) {
    for (int i = 0; i < CACHE; i++) {
        if (cache[i].block == block) {
            cache[i].block = 0;
            cache[i].dirty = 0;
            cache[i].used = 0;
        }
    }
}

// The superblock: bytes 1024-2047 of the device
static int write_super(void) {
    uint64_t first = 1024 / dev_bs, count = (1024 + dev_bs - 1) / dev_bs;
    uint32_t off = 1024 % dev_bs;
    if (count * dev_bs < off + 1024) count++;
    if (dev_rw(0, first, (uint32_t)count) < 0) return -EIO;
    memcpy(iobuf + off, sb, 1024);
    int r = dev_rw(1, first, (uint32_t)count);
    if (r == 0) sb_dirty = 0;
    return r;
}

static int flush(void) {
    write_pending = 0;
    for (;;) {
        struct cblk *lo = 0;
        for (int i = 0; i < CACHE; i++)
            if (cache[i].dirty && (!lo || cache[i].block < lo->block)) lo = &cache[i];
        if (!lo) break;
        if (write_cblk(lo) < 0) break;
    }
    if (sb_dirty && write_super() < 0) io_error = -EIO;
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_FLUSH;
    rx_block_call(blk, &q, 0, 0, 0, 0, 0, 0);
    int r = io_error;
    io_error = 0;
    return r;
}

static int64_t now_ns(void) { return rx_clock_get_utc(); }

// ---- group descriptors ----

static uint8_t *gd(uint32_t g, struct cblk **cs) {
    struct cblk *c = get_block(gdt_block + g * 32 / bs);
    if (!c) return 0;
    if (cs) *cs = c;
    return c->data + (g * 32) % bs;
}

static void sb_add(int off, int delta) {
    put32(sb + off, le32(sb + off) + (uint32_t)delta);
    sb_dirty = 1;
}

// ---- inodes ----

static uint8_t *ino_ptr(uint32_t ino, struct cblk **cs) {
    if (ino == 0 || ino > ninodes) return 0;
    uint32_t g = (ino - 1) / ipg, i = (ino - 1) % ipg;
    struct cblk *c;
    uint8_t *d = gd(g, &c);
    if (!d) return 0;
    uint32_t table = le32(d + 8);
    struct cblk *b = get_block(table + (uint32_t)((uint64_t)i * isz / bs));
    if (!b) return 0;
    if (cs) *cs = b;
    return b->data + ((uint64_t)i * isz) % bs;
}


static uint32_t i_mode(uint32_t ino) { uint8_t *p = ino_ptr(ino, 0); return p ? le16(p) : 0; }
static uint64_t i_size(uint32_t ino) {
    uint8_t *p = ino_ptr(ino, 0);
    if (!p) return 0;
    uint64_t s = le32(p + 4);
    if ((le16(p) & S_IFMT_) == S_IFREG_) s |= (uint64_t)le32(p + 108) << 32;
    return s;
}
static void set_size_field(uint32_t ino, uint64_t size) {
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return;
    put32(p + 4, (uint32_t)size);
    if ((le16(p) & S_IFMT_) == S_IFREG_) {
        put32(p + 108, (uint32_t)(size >> 32));
        if (size >= 0x80000000ull && !(feat_ro & RO_LARGE_FILE)) {
            feat_ro |= RO_LARGE_FILE;
            put32(sb + 100, feat_ro);
            sb_dirty = 1;
        }
    }
    c->dirty = 1;
}
static uint32_t i_links(uint32_t ino) { uint8_t *p = ino_ptr(ino, 0); return p ? le16(p + 26) : 0; }
static void set_links(uint32_t ino, uint32_t n) { struct cblk *c; uint8_t *p = ino_ptr(ino, &c); if (p) { put16(p + 26, n); c->dirty = 1; } }

// Times: seconds in the base inode; nanoseconds (and 2 epoch bits) in the
// extra part of 256-byte inodes
static int has_extra(const uint8_t *p, uint32_t field_end) { return isz > 128 && 128u + le16(p + 128) >= field_end; }
static int64_t get_time(const uint8_t *p, int base, int extra) {
    int64_t s = (int32_t)le32(p + base);
    int64_t ns = 0;
    if (extra && has_extra(p, (uint32_t)extra + 4)) {
        uint32_t x = le32(p + extra);
        s += (int64_t)(x & 3) << 32;
        ns = x >> 2;
    }
    return s * 1000000000 + ns;
}
static void put_time(uint8_t *p, int base, int extra, int64_t t) {
    int64_t s = t / 1000000000, ns = t % 1000000000;
    if (ns < 0) { ns += 1000000000; s--; }
    put32(p + base, (uint32_t)s);
    if (extra && has_extra(p, (uint32_t)extra + 4)) put32(p + extra, (uint32_t)(ns << 2) | (uint32_t)(((s - (int32_t)(uint32_t)s) >> 32) & 3));
}
#define T_ATIME 8, 140
#define T_CTIME 12, 132
#define T_MTIME 16, 136

static void touch(uint32_t ino, int m, int c) {
    struct cblk *cs;
    uint8_t *p = ino_ptr(ino, &cs);
    if (!p) return;
    int64_t t = now_ns();
    if (m) put_time(p, T_MTIME, t);
    if (c) put_time(p, T_CTIME, t);
    cs->dirty = 1;
}

static void add_iblocks(uint32_t ino, int64_t sectors) {
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return;
    uint64_t v = le32(p + 28) | (uint64_t)le16(p + 116) << 32;
    v += (uint64_t)sectors;
    put32(p + 28, (uint32_t)v);
    put16(p + 116, (uint32_t)(v >> 32));
    c->dirty = 1;
}

// ---- allocation ----

// A free block, near `goal` if possible; 0 when the disk is full
static uint32_t alloc_block(uint32_t goal) {
    if (le32(sb + 12) == 0) return 0;
    if (goal < first_data || goal >= nblocks) goal = first_data;
    uint32_t g0 = (goal - first_data) / bpg;
    for (uint32_t k = 0; k < ngroups; k++) {
        uint32_t g = (g0 + k) % ngroups;
        struct cblk *dc;
        uint8_t *d = gd(g, &dc);
        if (!d || le16(d + 12) == 0) continue;
        struct cblk *bm = get_block(le32(d));
        if (!bm) return 0;
        uint32_t in_group = g == ngroups - 1 ? nblocks - first_data - g * bpg : bpg;
        uint32_t start = k == 0 ? (goal - first_data) % bpg : 0;
        for (uint32_t step = 0; step < in_group; step++) {
            uint32_t i = (start + step) % in_group;
            if (bm->data[i / 8] & (1u << (i % 8))) continue;
            bm->data[i / 8] |= (uint8_t)(1u << (i % 8));
            bm->dirty = 1;
            put16(d + 12, le16(d + 12) - 1u);
            dc->dirty = 1;
            sb_add(12, -1);
            return first_data + g * bpg + i;
        }
    }
    return 0;
}

static void free_block(uint32_t b) {
    if (b < first_data || b >= nblocks) return;
    uint32_t g = (b - first_data) / bpg, i = (b - first_data) % bpg;
    struct cblk *dc;
    uint8_t *d = gd(g, &dc);
    if (!d) return;
    struct cblk *bm = get_block(le32(d));
    if (!bm || !(bm->data[i / 8] & (1u << (i % 8)))) return;
    bm->data[i / 8] &= (uint8_t)~(1u << (i % 8));
    bm->dirty = 1;
    put16(d + 12, le16(d + 12) + 1u);
    dc->dirty = 1;
    sb_add(12, 1);
    forget_block(b);
}

// A free inode: directories in the group with the most free inodes,
// other files near their directory
static uint32_t alloc_inode(uint32_t parent, int dir) {
    if (le32(sb + 16) == 0) return 0;
    uint32_t g0 = parent ? (parent - 1) / ipg : 0;
    if (dir) {
        uint32_t best = g0, most = 0;
        for (uint32_t g = 0; g < ngroups; g++) {
            uint8_t *d = gd(g, 0);
            if (d && le16(d + 14) > most) { most = le16(d + 14); best = g; }
        }
        g0 = best;
    }
    for (uint32_t k = 0; k < ngroups; k++) {
        uint32_t g = (g0 + k) % ngroups;
        struct cblk *dc;
        uint8_t *d = gd(g, &dc);
        if (!d || le16(d + 14) == 0) continue;
        struct cblk *bm = get_block(le32(d + 4));
        if (!bm) return 0;
        for (uint32_t i = 0; i < ipg; i++) {
            uint32_t ino = g * ipg + i + 1;
            if (ino < first_ino) continue;
            if (bm->data[i / 8] & (1u << (i % 8))) continue;
            bm->data[i / 8] |= (uint8_t)(1u << (i % 8));
            bm->dirty = 1;
            put16(d + 14, le16(d + 14) - 1u);
            if (dir) put16(d + 16, le16(d + 16) + 1u);
            dc->dirty = 1;
            sb_add(16, -1);
            return ino;
        }
    }
    return 0;
}

static void free_inode_bit(uint32_t ino, int dir) {
    uint32_t g = (ino - 1) / ipg, i = (ino - 1) % ipg;
    struct cblk *dc;
    uint8_t *d = gd(g, &dc);
    if (!d) return;
    struct cblk *bm = get_block(le32(d + 4));
    if (!bm || !(bm->data[i / 8] & (1u << (i % 8)))) return;
    bm->data[i / 8] &= (uint8_t)~(1u << (i % 8));
    bm->dirty = 1;
    put16(d + 14, le16(d + 14) + 1u);
    if (dir) put16(d + 16, le16(d + 16) - 1u);
    dc->dirty = 1;
    sb_add(16, 1);
}

// ---- block map (direct and indirect blocks) ----

static uint32_t per_block(void) { return bs / 4; }

// Physical block of logical block `lb` of an inode (0: a hole). With
// `alloc`, holes are filled (new blocks are zeroed when they hold
// pointers; data blocks are left to the caller) near `goal`. *fresh is set
// for a newly allocated data block. -1 on error (disk full).
static int64_t bmap(uint32_t ino, uint64_t lb, int alloc, uint32_t goal, int *fresh) {
    uint32_t per = per_block();
    if (fresh) *fresh = 0;
    struct cblk *ic;
    uint8_t *p = ino_ptr(ino, &ic);
    if (!p) return -1;
    if (!goal) goal = first_data + ((ino - 1) / ipg) * bpg;
    // Which pointer chain: direct, single, double or triple indirect
    int depth;
    uint32_t idx[4];
    if (lb < 12) { depth = 0; idx[0] = (uint32_t)lb; }
    else if ((lb -= 12) < per) { depth = 1; idx[0] = 12; idx[1] = (uint32_t)lb; }
    else if ((lb -= per) < (uint64_t)per * per) { depth = 2; idx[0] = 13; idx[1] = (uint32_t)(lb / per); idx[2] = (uint32_t)(lb % per); }
    else {
        lb -= (uint64_t)per * per;
        if (lb >= (uint64_t)per * per * per) return -1;
        depth = 3;
        idx[0] = 14;
        idx[1] = (uint32_t)(lb / ((uint64_t)per * per));
        idx[2] = (uint32_t)((lb / per) % per);
        idx[3] = (uint32_t)(lb % per);
    }
    uint32_t cur = le32(p + 40 + 4 * idx[0]);
    if (!cur) {
        if (!alloc) return 0;
        if (read_only) return -1;
        cur = alloc_block(goal);
        if (!cur) return -1;
        p = ino_ptr(ino, &ic);
        put32(p + 40 + 4 * idx[0], cur);
        ic->dirty = 1;
        add_iblocks(ino, bs / 512);
        if (depth) new_block(cur);
        else if (fresh) *fresh = 1;
    }
    for (int lv = 1; lv <= depth; lv++) {
        struct cblk *c = get_block(cur);
        if (!c) return -1;
        uint32_t next = le32(c->data + 4 * idx[lv]);
        if (!next) {
            if (!alloc) return 0;
            next = alloc_block(cur + 1);
            if (!next) return -1;
            c = get_block(cur);
            put32(c->data + 4 * idx[lv], next);
            c->dirty = 1;
            add_iblocks(ino, bs / 512);
            if (lv < depth) new_block(next);
            else if (fresh) *fresh = 1;
        }
        cur = next;
    }
    return cur;
}

// Free the blocks of a pointer block (depth 1: data blocks below it) from
// entry `from`; returns 1 if it is now empty (and freed it)
static int trunc_tree(uint32_t ino, uint32_t block, int depth, uint64_t from) {
    uint32_t per = per_block();
    uint64_t span = 1;
    for (int i = 1; i < depth; i++) span *= per;
    struct cblk *c = get_block(block);
    if (!c) return 0;
    int empty = 1;
    for (uint32_t i = 0; i < per; i++) {
        uint64_t lo = (uint64_t)i * span;
        c = get_block(block);
        if (!c) return 0;
        uint32_t b = le32(c->data + 4 * i);
        if (!b) continue;
        if (lo + span <= from) { empty = 0; continue; }   // entirely kept
        if (depth == 1) {
            free_block(b);
            add_iblocks(ino, -(int64_t)(bs / 512));
            c = get_block(block);
            put32(c->data + 4 * i, 0);
            c->dirty = 1;
        } else {
            uint64_t sub = from > lo ? from - lo : 0;
            if (trunc_tree(ino, b, depth - 1, sub)) {
                c = get_block(block);
                put32(c->data + 4 * i, 0);
                c->dirty = 1;
            } else {
                empty = 0;
            }
        }
    }
    if (empty) {
        free_block(block);
        add_iblocks(ino, -(int64_t)(bs / 512));
    }
    return empty;
}

// Free every block from logical block `from` on
static void free_from(uint32_t ino, uint64_t from) {
    uint32_t per = per_block();
    struct cblk *ic;
    uint8_t *p = ino_ptr(ino, &ic);
    if (!p) return;
    for (uint32_t i = 0; i < 12; i++) {
        uint32_t b = le32(p + 40 + 4 * i);
        if (b && i >= from) {
            free_block(b);
            add_iblocks(ino, -(int64_t)(bs / 512));
            p = ino_ptr(ino, &ic);
            put32(p + 40 + 4 * i, 0);
            ic->dirty = 1;
        }
    }
    uint64_t base = 12, span = per;
    for (int depth = 1; depth <= 3; depth++) {
        p = ino_ptr(ino, &ic);
        uint32_t b = le32(p + 40 + 4 * (11 + depth));
        if (b && base + span > from) {
            if (trunc_tree(ino, b, depth, from > base ? from - base : 0)) {
                p = ino_ptr(ino, &ic);
                put32(p + 40 + 4 * (11 + depth), 0);
                ic->dirty = 1;
            }
        }
        base += span;
        span *= per;
    }
}

// ---- file data ----

// Read or write `len` bytes at `off` (for a write, the caller extends the
// size). Runs of adjacent blocks go to the disk at once; holes read as
// zeros; a part of a block is read, changed and written.
static int file_io(uint32_t ino, uint64_t off, uint8_t *buf, size_t len, int write) {
    uint32_t last = 0;
    uint64_t known_fresh = UINT64_MAX;   // allocated by the run below, not yet written
    while (len) {
        uint64_t lb = off / bs;
        uint32_t in = (uint32_t)(off % bs);
        int fresh = 0;
        int64_t pb = bmap(ino, lb, write, last ? last + 1 : 0, &fresh);
        if (lb == known_fresh) fresh = 1;
        if (pb < 0) return write ? -ENOSPC : -EIO;
        if (pb == 0) {
            // A hole (reading only)
            size_t m = bs - in < len ? bs - in : len;
            memset(buf, 0, m);
            buf += m;
            off += m;
            len -= m;
            continue;
        }
        last = (uint32_t)pb;
        if (in || len < bs) {
            size_t m = bs - in < len ? bs - in : len;
            if (fresh) memset(iobuf, 0, bs);
            else if (dev_rw(0, blk_dev((uint32_t)pb), spb) < 0) return -EIO;
            if (write) {
                memcpy(iobuf + in, buf, m);
                if (dev_rw(1, blk_dev((uint32_t)pb), spb) < 0) return -EIO;
            } else {
                memcpy(buf, iobuf + in, m);
            }
            buf += m;
            off += m;
            len -= m;
            continue;
        }
        // Whole blocks: as many adjacent ones as fit
        uint32_t run = 1;
        while ((uint64_t)(run + 1) * bs <= len && (uint64_t)(run + 1) * bs <= IOBUF) {
            int f2 = 0;
            int64_t nb = bmap(ino, lb + run, write, (uint32_t)pb + run, &f2);
            if (nb != pb + run) {
                if (nb < 0 && write) return -ENOSPC;
                if (f2) known_fresh = lb + run;
                break;
            }
            run++;
        }
        size_t m = (size_t)run * bs;
        int r;
        if (write) {
            memcpy(iobuf, buf, m);
            r = dev_rw(1, blk_dev((uint32_t)pb), run * spb);
        } else {
            r = dev_rw(0, blk_dev((uint32_t)pb), run * spb);
            if (r == 0) memcpy(buf, iobuf, m);
        }
        if (r < 0) return -EIO;
        last = (uint32_t)pb + run - 1;
        buf += m;
        off += m;
        len -= m;
    }
    return 0;
}

// Set the size: blocks past it are freed and the rest of the last block is
// zeroed (so growing it later shows zeros); growing makes a hole
static int truncate_ino(uint32_t ino, uint64_t size) {
    uint64_t old = i_size(ino);
    if (size < old) {
        uint64_t keep = (size + bs - 1) / bs;
        free_from(ino, keep);
        uint32_t in = (uint32_t)(size % bs);
        if (in) {
            int64_t pb = bmap(ino, size / bs, 0, 0, 0);
            if (pb > 0 && dev_rw(0, blk_dev((uint32_t)pb), spb) == 0) {
                memset(iobuf + in, 0, bs - in);
                dev_rw(1, blk_dev((uint32_t)pb), spb);
            }
        }
    }
    set_size_field(ino, size);
    touch(ino, 1, 1);
    return 0;
}

// ---- directories ----

struct dent {
    uint32_t ino;
    uint8_t type;
    char name[256];
    uint32_t off;        // byte offset in the directory
};

static uint8_t type_of_mode(uint32_t mode) {
    switch (mode & S_IFMT_) {
    case S_IFREG_: return 1;
    case S_IFDIR_: return 2;
    case S_IFCHR_: return 3;
    case 0060000: return 4;
    case 0010000: return 5;
    case 0140000: return 6;
    case S_IFLNK_: return 7;
    }
    return 0;
}

static uint32_t name_len_of(const uint8_t *e) { return (feat_incompat & INCOMPAT_FILETYPE) ? e[6] : le16(e + 6); }

// The next entry at or after *pos: 1 (d filled, *pos past it), 0 at the end
static int dir_next(uint32_t dir, uint32_t *pos, struct dent *d) {
    uint64_t size = i_size(dir);
    while (*pos < size) {
        uint32_t lb = *pos / bs, in = *pos % bs;
        int64_t pb = bmap(dir, lb, 0, 0, 0);
        if (pb <= 0) { *pos = (lb + 1) * bs; continue; }
        struct cblk *c = get_block((uint32_t)pb);
        if (!c) return -EIO;
        uint8_t *e = c->data + in;
        uint32_t rec = le16(e + 4);
        if (rec < 8 || in + rec > bs || rec % 4) { *pos = (lb + 1) * bs; continue; }   // damaged: skip the block
        uint32_t nl = name_len_of(e);
        d->off = *pos;
        *pos += rec;
        uint32_t ino = le32(e);
        if (!ino || nl == 0 || 8 + nl > rec) continue;
        d->ino = ino;
        d->type = (feat_incompat & INCOMPAT_FILETYPE) ? e[7] : 0;
        memcpy(d->name, e + 8, nl);
        d->name[nl] = 0;
        return 1;
    }
    return 0;
}

static int dir_find(uint32_t dir, const char *name, size_t len, struct dent *d) {
    uint32_t pos = 0;
    int r;
    while ((r = dir_next(dir, &pos, d)) == 1)
        if (strlen(d->name) == len && !memcmp(d->name, name, len)) return 0;
    return r < 0 ? r : -ENOENT;
}

// A changed directory: no longer indexed (we do not keep the index)
static void dir_changed(uint32_t dir) {
    struct cblk *c;
    uint8_t *p = ino_ptr(dir, &c);
    if (p && (le32(p + 32) & INDEX_FL)) {
        put32(p + 32, le32(p + 32) & ~INDEX_FL);
        c->dirty = 1;
    }
    touch(dir, 1, 1);
}

static uint32_t rec_need(uint32_t nl) { return (8 + nl + 3) & ~3u; }

static int dir_add(uint32_t dir, const char *name, size_t len, uint32_t ino, uint8_t type) {
    uint32_t need = rec_need((uint32_t)len);
    uint64_t size = i_size(dir);
    for (uint32_t lb = 0; (uint64_t)lb * bs < size; lb++) {
        int64_t pb = bmap(dir, lb, 0, 0, 0);
        if (pb <= 0) continue;
        struct cblk *c = get_block((uint32_t)pb);
        if (!c) return -EIO;
        for (uint32_t in = 0; in < bs;) {
            uint8_t *e = c->data + in;
            uint32_t rec = le16(e + 4);
            if (rec < 8 || in + rec > bs) break;
            uint32_t used = le32(e) ? rec_need(name_len_of(e)) : 0;
            if (rec - used >= need) {
                uint8_t *n = e;
                if (used) {
                    put16(e + 4, used);
                    n = e + used;
                }
                put32(n, ino);
                put16(n + 4, rec - used);
                if (feat_incompat & INCOMPAT_FILETYPE) { n[6] = (uint8_t)len; n[7] = type; }
                else put16(n + 6, (uint32_t)len);
                memcpy(n + 8, name, len);
                c->dirty = 1;
                dir_changed(dir);
                return 0;
            }
            in += rec;
        }
    }
    // A new block at the end
    uint32_t lb = (uint32_t)(size / bs);
    int fresh;
    int64_t pb = bmap(dir, lb, 1, 0, &fresh);
    if (pb <= 0) return -ENOSPC;
    struct cblk *c = new_block((uint32_t)pb);
    put32(c->data, ino);
    put16(c->data + 4, bs);
    if (feat_incompat & INCOMPAT_FILETYPE) { c->data[6] = (uint8_t)len; c->data[7] = type; }
    else put16(c->data + 6, (uint32_t)len);
    memcpy(c->data + 8, name, len);
    set_size_field(dir, (uint64_t)(lb + 1) * bs);
    dir_changed(dir);
    return 0;
}

// Remove the entry at byte offset `off`: the one before it in the block
// takes its space (or, first in the block, it is marked unused)
static int dir_remove(uint32_t dir, uint32_t off) {
    uint32_t lb = off / bs, at = off % bs;
    int64_t pb = bmap(dir, lb, 0, 0, 0);
    if (pb <= 0) return -EIO;
    struct cblk *c = get_block((uint32_t)pb);
    if (!c) return -EIO;
    uint32_t prev = UINT32_MAX;
    for (uint32_t in = 0; in < at;) {
        uint32_t rec = le16(c->data + in + 4);
        if (rec < 8) return -EIO;
        prev = in;
        in += rec;
    }
    uint8_t *e = c->data + at;
    if (prev != UINT32_MAX) put16(c->data + prev + 4, le16(c->data + prev + 4) + le16(e + 4));
    else put32(e, 0);
    c->dirty = 1;
    dir_changed(dir);
    return 0;
}

// Point the entry at `off` to another inode
static int dir_retarget(uint32_t dir, uint32_t off, uint32_t ino, uint8_t type) {
    int64_t pb = bmap(dir, off / bs, 0, 0, 0);
    if (pb <= 0) return -EIO;
    struct cblk *c = get_block((uint32_t)pb);
    if (!c) return -EIO;
    uint8_t *e = c->data + off % bs;
    put32(e, ino);
    if (feat_incompat & INCOMPAT_FILETYPE) e[7] = type;
    c->dirty = 1;
    dir_changed(dir);
    return 0;
}

static int dir_empty(uint32_t dir) {
    uint32_t pos = 0;
    struct dent d;
    int r;
    while ((r = dir_next(dir, &pos, &d)) == 1)
        if (strcmp(d.name, ".") && strcmp(d.name, "..")) return 0;
    return r < 0 ? r : 1;
}

// ---- open inodes ----

struct onode {
    uint32_t ino;
    int refs;
    struct onode *next;
};

static struct onode *onodes;

static int open_count(uint32_t ino) {
    for (struct onode *n = onodes; n; n = n->next) if (n->ino == ino) return n->refs;
    return 0;
}

static void ino_ref(uint32_t ino) {
    for (struct onode *n = onodes; n; n = n->next) if (n->ino == ino) { n->refs++; return; }
    struct onode *n = calloc(1, sizeof *n);
    if (!n) return;
    n->ino = ino;
    n->refs = 1;
    n->next = onodes;
    onodes = n;
}

// An inode without names or openers goes: its blocks and its bitmap bit
static void maybe_free_inode(uint32_t ino) {
    if (i_links(ino) || open_count(ino) || read_only) return;
    uint32_t mode = i_mode(ino);
    if (!mode) return;
    uint8_t *p = ino_ptr(ino, 0);
    // Fast symbolic links keep their target in the block pointers
    int fast_link = (mode & S_IFMT_) == S_IFLNK_ && p && le32(p + 28) == 0;
    if (!fast_link) free_from(ino, 0);
    struct cblk *c;
    p = ino_ptr(ino, &c);
    if (!p) return;
    put32(p + 20, (uint32_t)(now_ns() / 1000000000));   // dtime
    if (fast_link) memset(p + 40, 0, 60);
    put32(p + 4, 0);
    c->dirty = 1;
    free_inode_bit(ino, (mode & S_IFMT_) == S_IFDIR_);
}

static void ino_unref(uint32_t ino) {
    for (struct onode **pp = &onodes; *pp; pp = &(*pp)->next) {
        struct onode *n = *pp;
        if (n->ino != ino) continue;
        if (--n->refs == 0) {
            *pp = n->next;
            free(n);
            maybe_free_inode(ino);
        }
        return;
    }
}

// A new inode: mode, owner root, times now, one link (two for directories)
static uint32_t make_inode(uint32_t parent, uint32_t mode) {
    int dir = (mode & S_IFMT_) == S_IFDIR_;
    uint32_t ino = alloc_inode(parent, dir);
    if (!ino) return 0;
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return 0;
    memset(p, 0, isz);
    put16(p, mode);
    if (isz > 128) {
        uint32_t want = le16(sb + 350) ? le16(sb + 350) : 32;
        if (want < 32) want = 32;
        if (128 + want > isz) want = isz - 128;
        put16(p + 128, want);
    }
    int64_t t = now_ns();
    put_time(p, T_ATIME, t);
    put_time(p, T_CTIME, t);
    put_time(p, T_MTIME, t);
    if (isz > 128 && has_extra(p, 152)) put_time(p, 144, 148, t);   // creation time
    put16(p + 26, dir ? 2 : 1);
    uint32_t gen;
    if (getrandom(&gen, 4, 0) != 4) gen = (uint32_t)t;
    put32(p + 100, gen);
    c->dirty = 1;
    return ino;
}

// ---- paths ----

static char g_redir[PATHMAX + 2];
static size_t g_redir_len;

static int is_dir_ino(uint32_t ino) { return (i_mode(ino) & S_IFMT_) == S_IFDIR_; }
static int is_link_ino(uint32_t ino) { return (i_mode(ino) & S_IFMT_) == S_IFLNK_; }

static int read_link(uint32_t ino, char *out, size_t cap, size_t *len) {
    uint64_t size = i_size(ino);
    if (size >= cap || size >= PATHMAX) return -ENAMETOOLONG;
    uint8_t *p = ino_ptr(ino, 0);
    if (!p) return -EIO;
    uint64_t blocks = le32(p + 28);
    uint32_t acl = le32(p + 104);
    if (blocks == 0 || (acl && blocks == bs / 512)) {
        if (size > 60) return -EIO;
        memcpy(out, p + 40, size);
    } else {
        int r = file_io(ino, 0, (uint8_t *)out, size, 0);
        if (r < 0) return r;
    }
    *len = size;
    return 0;
}

// Follow a link met in directory `prefix` (a path from our root, plen
// bytes): the redirect payload is its target, then the rest of the path
static int redirect(uint32_t link, const char *prefix, size_t plen, const char *rest, size_t rlen, int which) {
    char target[PATHMAX];
    size_t tl;
    int r = read_link(link, target, sizeof target, &tl);
    if (r < 0) return r;
    if (tl == 0) return -ENOENT;
    size_t used = 0;
    g_redir[used++] = (char)which;
    if (target[0] != '/' && plen) {
        if (used + plen + 1 >= sizeof g_redir) return -ENAMETOOLONG;
        memcpy(g_redir + used, prefix, plen);
        used += plen;
        g_redir[used++] = '/';
    }
    if (used + tl + 1 + rlen >= sizeof g_redir) return -ENAMETOOLONG;
    memcpy(g_redir + used, target, tl);
    used += tl;
    if (rlen) {
        g_redir[used++] = '/';
        memcpy(g_redir + used, rest, rlen);
        used += rlen;
    }
    g_redir_len = used;
    return RX_FS_REDIRECT;
}

struct ofd;
static const char *ofd_path(struct ofd *o, size_t *len);

// Walk `path` from directory `start` (whose path from our root is
// `base`). Links on the way are followed (a redirect), and at the end if
// `follow`. With `parent`: stop before the last component.
static int walk(uint32_t start, const char *base, size_t blen, const char *path, size_t len, int follow,
                uint32_t *out, uint32_t *parent, const char **last, size_t *last_len, int which) {
    uint32_t cur = start;
    char pre[PATHMAX];
    size_t pl = 0;
    if (blen >= sizeof pre) return -ENAMETOOLONG;
    memcpy(pre, base, blen);
    pl = blen;
    size_t i = 0;
    while (i < len && path[i] == '/') i++;
    if (i == len) {
        if (parent) return -EEXIST;
        *out = cur;
        return 0;
    }
    for (;;) {
        size_t j = i;
        while (j < len && path[j] != '/') j++;
        size_t k = j;
        while (k < len && path[k] == '/') k++;
        int final = k == len;
        size_t nl = j - i;
        if (nl > 255) return -ENAMETOOLONG;
        if ((nl == 1 && path[i] == '.') || (nl == 2 && path[i] == '.' && path[i + 1] == '.')) return -EINVAL;
        if (!is_dir_ino(cur)) return -ENOTDIR;
        if (final && parent) {
            *parent = cur;
            *last = path + i;
            *last_len = nl;
            return 0;
        }
        struct dent d;
        int r = dir_find(cur, path + i, nl, &d);
        if (r < 0) return r;
        if (is_link_ino(d.ino) && (!final || follow)) return redirect(d.ino, pre, pl, path + k, len - k, which);
        if (final) {
            *out = d.ino;
            return 0;
        }
        if (pl + 1 + nl >= sizeof pre) return -ENAMETOOLONG;
        if (pl) pre[pl++] = '/';
        memcpy(pre + pl, path + i, nl);
        pl += nl;
        cur = d.ino;
        i = k;
    }
}

static void attr_of(uint32_t ino, rx_fs_attr_t *a) {
    memset(a, 0, sizeof *a);
    uint8_t *p = ino_ptr(ino, 0);
    if (!p) return;
    uint32_t mode = le16(p);
    switch (mode & S_IFMT_) {
    case S_IFDIR_: a->type = RX_FS_TYPE_DIR; break;
    case S_IFLNK_: a->type = RX_FS_TYPE_SYMLINK; break;
    case S_IFCHR_: a->type = RX_FS_TYPE_CHR; break;
    default: a->type = RX_FS_TYPE_FILE; break;
    }
    a->mode = mode & 07777;
    a->size = i_size(ino);
    a->ino = ino;
    a->nlink = le16(p + 26);
    a->mtime_ns = get_time(p, T_MTIME);
    a->atime_ns = get_time(p, T_ATIME);
    a->ctime_ns = get_time(p, T_CTIME);
    if (!a->ctime_ns) a->ctime_ns = 1;   // says: these fields are filled
    a->uid = le16(p + 2) | (uint32_t)le16(p + 120) << 16;
    a->gid = le16(p + 24) | (uint32_t)le16(p + 122) << 16;
    a->blocks = le32(p + 28) | (uint64_t)le16(p + 116) << 32;
    a->read_only = (uint32_t)read_only;
    a->fsid = fsid;
}

// ---- connections ----

struct ofd {
    uint32_t ino;
    uint64_t offset;
    int flags;
    uint32_t dir_pos;
    int refs;
    char *path;          // from our root (for links met below a directory)
};

struct conn {
    rx_handle_t ch;
    struct ofd *o;
};

static struct conn conns[MAX_CONNS];
static int nconns;

static const char *ofd_path(struct ofd *o, size_t *len) {
    *len = o->path ? strlen(o->path) : 0;
    return o->path ? o->path : "";
}

static struct ofd *new_ofd(uint32_t ino, int flags, const char *path, size_t plen) {
    struct ofd *o = calloc(1, sizeof *o);
    if (!o) return 0;
    o->ino = ino;
    o->flags = flags;
    o->path = strndup(path, plen);
    ino_ref(ino);
    return o;
}

// The caller still owns ch if this fails
static int add_conn(rx_handle_t ch, struct ofd *o) {
    if (nconns == MAX_CONNS) return -EMFILE;
    conns[nconns].ch = ch;
    conns[nconns++].o = o;
    o->refs++;
    return 0;
}

static void drop_conn(int i) {
    struct ofd *o = conns[i].o;
    rx_handle_close(conns[i].ch);
    conns[i] = conns[--nconns];
    if (--o->refs == 0) {
        ino_unref(o->ino);
        free(o->path);
        free(o);
        if (!read_only) write_pending = 1;
    }
    if (write_pending) flush();
}

// ---- requests ----

static char req[sizeof(rx_fs_hdr_t) + RX_FS_MAX_IO + 8192];
static char rep[sizeof(rx_fs_reply_t) + RX_FS_MAX_IO + PATHMAX + 64];

static int walk_o(struct ofd *o, const char *path, size_t plen, int follow, uint32_t *out, uint32_t *parent,
                  const char **last, size_t *ll, int which) {
    size_t bl;
    const char *base = ofd_path(o, &bl);
    return walk(o->ino, base, bl, path, plen, follow, out, parent, last, ll, which);
}
#define WALK(o, path, plen, follow, out, parent, last, ll, which) walk_o(o, path, plen, follow, out, parent, last, ll, which)

static int do_open(struct ofd *dir, const char *path, size_t plen, uint32_t flags, uint32_t mode,
                   rx_handle_t server, uint32_t *type) {
    uint32_t ino = 0, parent;
    const char *last;
    size_t ll;
    int follow = !(flags & O_NOFOLLOW) && !((flags & O_CREAT) && (flags & O_EXCL));
    int r = WALK(dir, path, plen, follow, &ino, 0, 0, 0, 0);
    if (r == RX_FS_REDIRECT) return r;
    if (r == -ENOENT && (flags & O_CREAT)) {
        if (read_only) return -EROFS;
        r = WALK(dir, path, plen, 1, 0, &parent, &last, &ll, 0);
        if (r) return r;
        struct dent d;
        if (dir_find(parent, last, ll, &d) == 0) return -EEXIST;
        ino = make_inode(parent, S_IFREG_ | (mode & 07777));
        if (!ino) return -ENOSPC;
        r = dir_add(parent, last, ll, ino, 1);
        if (r < 0) {
            set_links(ino, 0);
            maybe_free_inode(ino);
            return r;
        }
    } else if (r < 0) {
        return r;
    } else if ((flags & O_CREAT) && (flags & O_EXCL)) {
        return -EEXIST;
    }
    uint32_t m = i_mode(ino);
    if ((m & S_IFMT_) == S_IFLNK_) return -ELOOP;   // O_NOFOLLOW
    int writing = (flags & O_ACCMODE) != O_RDONLY;
    int isdir = (m & S_IFMT_) == S_IFDIR_;
    if (isdir && writing) return -EISDIR;
    if ((flags & O_DIRECTORY) && !isdir) return -ENOTDIR;
    if ((writing || (flags & O_TRUNC)) && read_only) return -EROFS;
    if ((flags & O_TRUNC) && writing && !isdir && i_size(ino)) truncate_ino(ino, 0);
    struct ofd *o = new_ofd(ino, (int)flags, path, plen);
    if (!o) return -ENOMEM;
    // Its path from our root: the directory's, then the path
    size_t dl;
    const char *dp = ofd_path(dir, &dl);
    if (dl) {
        char *full = malloc(dl + 1 + plen + 1);
        if (full) {
            memcpy(full, dp, dl);
            full[dl] = '/';
            memcpy(full + dl + 1, path, plen);
            full[dl + 1 + plen] = 0;
            free(o->path);
            o->path = full;
        }
    }
    r = add_conn(server, o);
    if (r < 0) {
        ino_unref(ino);
        free(o->path);
        free(o);
        return r;
    }
    *type = isdir ? RX_FS_TYPE_DIR : (m & S_IFMT_) == S_IFCHR_ ? RX_FS_TYPE_CHR : RX_FS_TYPE_FILE;
    return 0;
}

static int do_read(struct ofd *o, uint32_t count, char *out, size_t *len) {
    uint32_t m = i_mode(o->ino);
    if ((m & S_IFMT_) == S_IFDIR_) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_WRONLY) return -EBADF;
    if (count > RX_FS_MAX_IO) count = RX_FS_MAX_IO;
    uint64_t size = i_size(o->ino);
    uint64_t avail = o->offset < size ? size - o->offset : 0;
    size_t n = count < avail ? count : (size_t)avail;
    if (n) {
        int r = file_io(o->ino, o->offset, (uint8_t *)out, n, 0);
        if (r < 0) return r;
    }
    o->offset += n;
    *len = n;
    // Access time as Linux's relatime: when older than the last change or a day
    if (!read_only && n) {
        struct cblk *c;
        uint8_t *p = ino_ptr(o->ino, &c);
        if (p) {
            int64_t a = get_time(p, T_ATIME), mt = get_time(p, T_MTIME), ct = get_time(p, T_CTIME), t = now_ns();
            if (a <= mt || a <= ct || t - a > 86400ll * 1000000000) {
                put_time(p, T_ATIME, t);
                c->dirty = 1;
                write_pending = 1;
            }
        }
    }
    return 0;
}

static int do_write(struct ofd *o, const char *data, size_t k, uint64_t *wrote) {
    uint32_t m = i_mode(o->ino);
    if ((m & S_IFMT_) == S_IFDIR_) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_RDONLY) return -EBADF;
    if (read_only) return -EROFS;
    uint64_t size = i_size(o->ino);
    if (o->flags & O_APPEND) o->offset = size;
    uint64_t max = (uint64_t)per_block() * per_block() * per_block() * bs;
    if (o->offset + k > max || o->offset + k < o->offset) return -EFBIG;
    if (k) {
        int r = file_io(o->ino, o->offset, (uint8_t *)data, k, 1);
        if (r < 0) {
            // Disk full part-way: nothing of this write is kept past the
            // old end (no blocks beyond the size)
            free_from(o->ino, (size + bs - 1) / bs);
            return r;
        }
    }
    o->offset += k;
    if (o->offset > size) set_size_field(o->ino, o->offset);
    touch(o->ino, 1, 1);
    *wrote = k;
    return 0;
}

static int do_readdir(struct ofd *o, uint32_t max, char *out, size_t *len) {
    if (!is_dir_ino(o->ino)) return -ENOTDIR;
    if (max > RX_FS_MAX_IO) max = RX_FS_MAX_IO;
    size_t used = 0;
    for (;;) {
        uint32_t pos = o->dir_pos;
        struct dent d;
        int r = dir_next(o->ino, &pos, &d);
        if (r < 0) return r;
        if (r == 0) break;
        if (!strcmp(d.name, ".") || !strcmp(d.name, "..")) {
            o->dir_pos = pos;
            continue;
        }
        size_t nl = strlen(d.name);
        if (used + 10 + nl > max) break;
        uint64_t ino = d.ino;
        uint8_t t = d.type;
        if (!(feat_incompat & INCOMPAT_FILETYPE)) t = type_of_mode(i_mode(d.ino));
        memcpy(out + used, &ino, 8);
        out[used + 8] = (char)(t == 2 ? RX_FS_TYPE_DIR : t == 7 ? RX_FS_TYPE_SYMLINK : t == 3 ? RX_FS_TYPE_CHR : RX_FS_TYPE_FILE);
        out[used + 9] = (char)nl;
        memcpy(out + used + 10, d.name, nl);
        used += 10 + nl;
        o->dir_pos = pos;
    }
    *len = used;
    return 0;
}

static int valid_name(const char *s, size_t n) {
    if (!n || n > 255) return 0;
    if ((n == 1 && s[0] == '.') || (n == 2 && s[0] == '.' && s[1] == '.')) return 0;
    return !memchr(s, 0, n) && !memchr(s, '/', n);
}

static int do_mkdir(struct ofd *dir, const char *path, size_t plen, uint32_t mode) {
    uint32_t parent;
    const char *last;
    size_t ll;
    if (read_only) return -EROFS;
    int r = WALK(dir, path, plen, 1, 0, &parent, &last, &ll, 0);
    if (r) return r;
    if (!valid_name(last, ll)) return -EINVAL;
    struct dent d;
    if (dir_find(parent, last, ll, &d) == 0) return -EEXIST;
    if (i_links(parent) >= 65000) return -EMLINK;
    uint32_t ino = make_inode(parent, S_IFDIR_ | (mode & 07777));
    if (!ino) return -ENOSPC;
    int fresh;
    int64_t pb = bmap(ino, 0, 1, 0, &fresh);
    if (pb <= 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return -ENOSPC;
    }
    struct cblk *c = new_block((uint32_t)pb);
    put32(c->data, ino);
    put16(c->data + 4, 12);
    c->data[6] = 1;
    c->data[7] = (feat_incompat & INCOMPAT_FILETYPE) ? 2 : 0;
    c->data[8] = '.';
    put32(c->data + 12, parent);
    put16(c->data + 16, bs - 12);
    c->data[18] = 2;
    c->data[19] = (feat_incompat & INCOMPAT_FILETYPE) ? 2 : 0;
    c->data[20] = '.';
    c->data[21] = '.';
    set_size_field(ino, bs);
    r = dir_add(parent, last, ll, ino, 2);
    if (r < 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return r;
    }
    set_links(parent, i_links(parent) + 1);
    return 0;
}

// Take a name away from an inode (`isdir`: a directory's own "." and its
// parent's ".." link go too)
static void drop_name(uint32_t parent, uint32_t ino, int isdir) {
    if (isdir) {
        set_links(ino, 0);
        set_links(parent, i_links(parent) > 2 ? i_links(parent) - 1 : 2);
    } else {
        uint32_t l = i_links(ino);
        set_links(ino, l ? l - 1 : 0);
    }
    touch(ino, 0, 1);
    maybe_free_inode(ino);
}

static int do_unlink(struct ofd *dir, const char *path, size_t plen, uint32_t flags) {
    uint32_t parent;
    const char *last;
    size_t ll;
    if (read_only) return -EROFS;
    int r = WALK(dir, path, plen, 1, 0, &parent, &last, &ll, 0);
    if (r) return r;
    struct dent d;
    r = dir_find(parent, last, ll, &d);
    if (r < 0) return r;
    int isdir = is_dir_ino(d.ino);
    if (flags & RX_FS_UNLINK_DIR) {
        if (!isdir) return -ENOTDIR;
        r = dir_empty(d.ino);
        if (r <= 0) return r < 0 ? r : -ENOTEMPTY;
        if (d.ino == 2) return -EBUSY;
    } else if (isdir) {
        return -EISDIR;
    }
    r = dir_remove(parent, d.off);
    if (r < 0) return r;
    drop_name(parent, d.ino, isdir);
    return 0;
}

// Is directory `a` the same as `b` or above it? (following "..")
static int is_ancestor(uint32_t a, uint32_t b) {
    for (int guard = 0; guard < 4096; guard++) {
        if (a == b) return 1;
        if (b == 2) return 0;
        struct dent d;
        if (dir_find(b, "..", 2, &d) < 0) return 0;
        if (d.ino == b) return 0;
        b = d.ino;
    }
    return 1;
}

static int do_rename(struct ofd *dir, const char *a, size_t alen, const char *b, size_t blen) {
    uint32_t pa, pb;
    const char *na, *nb;
    size_t la, lb;
    if (read_only) return -EROFS;
    int r = WALK(dir, a, alen, 1, 0, &pa, &na, &la, 0);
    if (r) return r;
    r = WALK(dir, b, blen, 1, 0, &pb, &nb, &lb, 1);
    if (r) return r;
    if (!valid_name(nb, lb)) return -EINVAL;
    struct dent ds, dd;
    r = dir_find(pa, na, la, &ds);
    if (r < 0) return r;
    if (ds.ino == 2) return -EBUSY;
    int isdir = is_dir_ino(ds.ino);
    if (isdir && is_ancestor(ds.ino, pb)) return -EINVAL;
    int exists = dir_find(pb, nb, lb, &dd) == 0;
    uint8_t type = type_of_mode(i_mode(ds.ino));
    if (exists) {
        if (dd.ino == ds.ino) return 0;   // two names of one file: nothing to do
        int ddir = is_dir_ino(dd.ino);
        if (ddir && !isdir) return -EISDIR;
        if (!ddir && isdir) return -ENOTDIR;
        if (ddir && (r = dir_empty(dd.ino)) <= 0) return r < 0 ? r : -ENOTEMPTY;
        r = dir_retarget(pb, dd.off, ds.ino, type);
        if (r < 0) return r;
        drop_name(pb, dd.ino, ddir);
    } else {
        if (isdir && pa != pb && i_links(pb) >= 65000) return -EMLINK;
        r = dir_add(pb, nb, lb, ds.ino, type);
        if (r < 0) return r;
    }
    // The old entry (found again: adding may have moved it within its block)
    if (dir_find(pa, na, la, &ds) == 0) dir_remove(pa, ds.off);
    if (isdir && pa != pb) {
        // Its ".." now names the new parent
        struct dent dot;
        if (dir_find(ds.ino, "..", 2, &dot) == 0) dir_retarget(ds.ino, dot.off, pb, 2);
        set_links(pb, i_links(pb) + 1);
        set_links(pa, i_links(pa) > 2 ? i_links(pa) - 1 : 2);
    }
    touch(ds.ino, 0, 1);
    return 0;
}

static int do_link(struct ofd *dir, const char *a, size_t alen, const char *b, size_t blen) {
    uint32_t ino, pb;
    const char *nb;
    size_t lb;
    if (read_only) return -EROFS;
    int r = WALK(dir, a, alen, 0, &ino, 0, 0, 0, 0);
    if (r) return r;
    r = WALK(dir, b, blen, 1, 0, &pb, &nb, &lb, 1);
    if (r) return r;
    if (!valid_name(nb, lb)) return -EINVAL;
    if (is_dir_ino(ino)) return -EPERM;
    if (i_links(ino) >= 65000) return -EMLINK;
    struct dent d;
    if (dir_find(pb, nb, lb, &d) == 0) return -EEXIST;
    r = dir_add(pb, nb, lb, ino, type_of_mode(i_mode(ino)));
    if (r < 0) return r;
    set_links(ino, i_links(ino) + 1);
    touch(ino, 0, 1);
    return 0;
}

static int do_symlink(struct ofd *dir, const char *body, size_t blen) {
    uint32_t tl, parent;
    const char *last;
    size_t ll;
    if (blen < 4) return -EINVAL;
    memcpy(&tl, body, 4);
    if (!tl || tl > blen - 4 || tl >= PATHMAX || tl >= bs) return tl >= bs ? -ENAMETOOLONG : -EINVAL;
    if (read_only) return -EROFS;
    const char *target = body + 4, *path = body + 4 + tl;
    int r = WALK(dir, path, blen - 4 - tl, 1, 0, &parent, &last, &ll, 0);
    if (r) return r;
    if (!valid_name(last, ll)) return -EINVAL;
    struct dent d;
    if (dir_find(parent, last, ll, &d) == 0) return -EEXIST;
    uint32_t ino = make_inode(parent, S_IFLNK_ | 0777);
    if (!ino) return -ENOSPC;
    if (tl < 60) {
        // A fast link: the target in the block pointers
        struct cblk *c;
        uint8_t *p = ino_ptr(ino, &c);
        memcpy(p + 40, target, tl);
        c->dirty = 1;
    } else if ((r = file_io(ino, 0, (uint8_t *)target, tl, 1)) < 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return r;
    }
    set_size_field(ino, tl);
    r = dir_add(parent, last, ll, ino, 7);
    if (r < 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
    }
    return r;
}

static void statfs_of(rx_fs_statfs_t *s) {
    memset(s, 0, sizeof *s);
    s->block_size = bs;
    s->blocks = nblocks - first_data;
    s->blocks_free = le32(sb + 12);
    s->files = ninodes;
    s->files_free = le32(sb + 16);
    s->name_max = 255;
    s->flags = read_only ? RX_FS_MOUNT_RDONLY : 0;
    strcpy(s->type, "ext2");
}

static int shutting_down;

static int serve(int i) {
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_status_t s = rx_channel_read(conns[i].ch, 0, req, &h, sizeof req, 1, &nb, &nh);
    if (s == RX_ERR_SHOULD_WAIT) return 0;
    if (s != RX_OK) {
        drop_conn(i);
        return -1;
    }
    if (!nh) h = 0;
    if (nb < sizeof(rx_fs_hdr_t)) {
        if (h) rx_handle_close(h);
        return 0;
    }
    rx_fs_hdr_t hdr;
    memcpy(&hdr, req, sizeof hdr);
    const char *body = req + sizeof hdr;
    size_t blen = nb - sizeof hdr;
    struct ofd *o = conns[i].o;
    char *out = rep + sizeof(rx_fs_reply_t);
    size_t olen = 0;
    int st = 0, changed = 0;
    uint32_t u32;
    rx_handle_t back = 0;   // returned with the reply (a redirected OPEN)

    switch (hdr.op) {
    case RX_FS_OP_OPEN: {
        if (blen < 8 || !h) { st = -EINVAL; break; }
        uint32_t f[2];
        memcpy(f, body, 8);
        if (!is_dir_ino(o->ino)) { st = -ENOTDIR; break; }
        st = do_open(o, body + 8, blen - 8, f[0], f[1], h, &u32);
        if (st == RX_FS_REDIRECT) back = h;   // fsd tries again with it
        else if (st == 0) {
            memcpy(out, &u32, 4);
            olen = 4;
        } else {
            rx_handle_close(h);
        }
        h = 0;
        changed = f[0] & (O_CREAT | O_TRUNC) ? 1 : 0;
        break;
    }
    case RX_FS_OP_CLONE:
        if (!h) { st = -EINVAL; break; }
        st = add_conn(h, o);
        if (st == 0) h = 0;
        break;
    case RX_FS_OP_STAT:
        attr_of(o->ino, (rx_fs_attr_t *)out);
        olen = sizeof(rx_fs_attr_t);
        break;
    case RX_FS_OP_STAT_AT:
    case RX_FS_OP_LSTAT_AT: {
        uint32_t ino;
        st = WALK(o, body, blen, hdr.op == RX_FS_OP_STAT_AT, &ino, 0, 0, 0, 0);
        if (st == 0) {
            attr_of(ino, (rx_fs_attr_t *)out);
            olen = sizeof(rx_fs_attr_t);
        }
        break;
    }
    case RX_FS_OP_READLINK: {
        uint32_t ino;
        st = WALK(o, body, blen, 0, &ino, 0, 0, 0, 0);
        if (st == 0) st = is_link_ino(ino) ? read_link(ino, out, PATHMAX, &olen) : -EINVAL;
        break;
    }
    case RX_FS_OP_READ:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_read(o, u32, out, &olen);
        break;
    case RX_FS_OP_WRITE: {
        uint64_t wrote = 0;
        st = do_write(o, body, blen > RX_FS_MAX_IO ? RX_FS_MAX_IO : blen, &wrote);
        memcpy(out, &wrote, 8);
        olen = 8;
        changed = 2;
        break;
    }
    case RX_FS_OP_SEEK: {
        if (blen < 12) { st = -EINVAL; break; }
        int64_t off;
        memcpy(&off, body, 8);
        memcpy(&u32, body + 8, 4);
        int64_t base = u32 == SEEK_SET ? 0 : u32 == SEEK_CUR ? (int64_t)o->offset : u32 == SEEK_END ? (int64_t)i_size(o->ino) : -1;
        if (base < 0 || base + off < 0) { st = -EINVAL; break; }
        o->offset = (uint64_t)(base + off);
        memcpy(out, &o->offset, 8);
        olen = 8;
        break;
    }
    case RX_FS_OP_TRUNCATE: {
        uint64_t len;
        if (blen < 8) { st = -EINVAL; break; }
        memcpy(&len, body, 8);
        if (is_dir_ino(o->ino)) st = -EISDIR;
        else if (read_only) st = -EROFS;
        else if ((o->flags & O_ACCMODE) == O_RDONLY) st = -EBADF;
        else st = truncate_ino(o->ino, len);
        changed = 1;
        break;
    }
    case RX_FS_OP_SETATTR: {
        rx_fs_setattr_t sa;
        memset(&sa, 0, sizeof sa);
        if (blen < 16) { st = -EINVAL; break; }
        memcpy(&sa, body, blen < sizeof sa ? blen : sizeof sa);
        if (read_only) { st = -EROFS; break; }
        struct cblk *c;
        uint8_t *p = ino_ptr(o->ino, &c);
        if (!p) { st = -EIO; break; }
        if (sa.mask & RX_FS_SETATTR_MODE) put16(p, (le16(p) & S_IFMT_) | (sa.mode & 07777));
        if (sa.mask & RX_FS_SETATTR_MTIME) put_time(p, T_MTIME, sa.mtime_ns);
        if (sa.mask & RX_FS_SETATTR_ATIME) put_time(p, T_ATIME, sa.atime_ns);
        if (sa.mask & RX_FS_SETATTR_UID) { put16(p + 2, sa.uid); put16(p + 120, sa.uid >> 16); }
        if (sa.mask & RX_FS_SETATTR_GID) { put16(p + 24, sa.gid); put16(p + 122, sa.gid >> 16); }
        put_time(p, T_CTIME, now_ns());
        c->dirty = 1;
        changed = 1;
        break;
    }
    case RX_FS_OP_READDIR:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_readdir(o, u32, out, &olen);
        break;
    case RX_FS_OP_REWIND:
        o->dir_pos = 0;
        break;
    case RX_FS_OP_MKDIR:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_mkdir(o, body + 4, blen - 4, u32);
        changed = 1;
        break;
    case RX_FS_OP_UNLINK:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_unlink(o, body + 4, blen - 4, u32);
        changed = 1;
        break;
    case RX_FS_OP_RENAME:
    case RX_FS_OP_LINK:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        if (u32 > blen - 4) { st = -EINVAL; break; }
        st = hdr.op == RX_FS_OP_RENAME ? do_rename(o, body + 4, u32, body + 4 + u32, blen - 4 - u32)
                                       : do_link(o, body + 4, u32, body + 4 + u32, blen - 4 - u32);
        changed = 1;
        break;
    case RX_FS_OP_SYMLINK:
        st = do_symlink(o, body, blen);
        changed = 1;
        break;
    case RX_FS_OP_SYNC:
        st = flush();
        break;
    case RX_FS_OP_STATFS:
        statfs_of((rx_fs_statfs_t *)out);
        olen = sizeof(rx_fs_statfs_t);
        break;
    case RX_FS_OP_SHUTDOWN:
        if (blen >= 4) memcpy(&u32, body, 4);
        else u32 = 0;
        if (nconns > 1 && !(u32 & RX_FS_UNMOUNT_FORCE)) { st = -EBUSY; break; }
        if (!read_only) {
            // Cleanly unmounted
            put16(sb + 58, le16(sb + 58) | 1);
            put32(sb + 48, (uint32_t)(now_ns() / 1000000000));
            sb_dirty = 1;
        }
        if (flush() < 0) {
            fprintf(stderr, "ext2fs: %s: could not write everything out\n", dev_name);
            st = -EIO;
        }
        read_only = 1;
        shutting_down = 1;
        break;
    default:
        st = -ENOSYS;
        break;
    }
    if (st == RX_FS_REDIRECT) {
        memcpy(out, g_redir, g_redir_len);
        olen = g_redir_len;
        changed = 0;
    }
    if (changed == 2 && !read_only) write_pending = 1;
    else if (changed && !read_only && st >= 0) {
        int f = flush();
        if (st >= 0 && f < 0) st = f;
    } else if (changed && !read_only) {
        flush();
    }
    if (h) rx_handle_close(h);
    rx_fs_reply_t r = { hdr.txid, st };
    memcpy(rep, &r, sizeof r);
    if (rx_channel_write(conns[i].ch, 0, rep, (uint32_t)(sizeof r + (st < 0 ? 0 : olen)), back ? &back : 0, back ? 1 : 0) != RX_OK && back)
        rx_handle_close(back);
    return 0;
}

// ---- mounting ----

static int load(void) {
    // The superblock: 1024 bytes at byte 1024
    uint64_t first = 1024 / dev_bs, count = (2048 + dev_bs - 1) / dev_bs - first;
    if (dev_rw(0, first, (uint32_t)count) < 0) return -EIO;
    memcpy(sb, iobuf + 1024 % dev_bs, 1024);
    if (le16(sb + 56) != 0xEF53) return -EINVAL;
    if (le32(sb + 76) > 1) return -EINVAL;   // revision
    uint32_t log = le32(sb + 24);
    if (log > 2) return -EINVAL;   // 1, 2 or 4 KiB blocks
    bs = 1024u << log;
    if (bs < dev_bs || bs % dev_bs) {
        fprintf(stderr, "ext2fs: %s: blocks of %u bytes on a disk with %u-byte sectors\n", dev_name, bs, dev_bs);
        return -EINVAL;
    }
    spb = bs / dev_bs;
    nblocks = le32(sb + 4);
    ninodes = le32(sb);
    first_data = le32(sb + 20);
    bpg = le32(sb + 32);
    ipg = le32(sb + 40);
    if (!bpg || !ipg || bpg > bs * 8 || ipg > bs * 8 || nblocks <= first_data) return -EINVAL;
    if ((uint64_t)nblocks * spb > dev_blocks) return -EINVAL;
    ngroups = (nblocks - first_data + bpg - 1) / bpg;
    if ((uint64_t)ngroups * ipg < ninodes) return -EINVAL;
    if (le32(sb + 76) >= 1) {
        isz = le16(sb + 88);
        first_ino = le32(sb + 84);
        feat_compat = le32(sb + 92);
        feat_incompat = le32(sb + 96);
        feat_ro = le32(sb + 100);
    } else {
        isz = 128;
        first_ino = 11;
    }
    if (isz < 128 || isz > bs || (isz & (isz - 1))) return -EINVAL;
    gdt_block = first_data + 1;
    uint32_t unknown = feat_incompat & ~INCOMPAT_FILETYPE;
    if (unknown) {
        fprintf(stderr, "ext2fs: %s: has features this server cannot read (incompat %#x: ext4, step 16)\n", dev_name, unknown);
        return -ENOTSUP;
    }
    if ((feat_ro & ~(RO_SPARSE_SUPER | RO_LARGE_FILE)) || (feat_compat & COMPAT_HAS_JOURNAL)) {
        if (!read_only) fprintf(stderr, "ext2fs: %s: has ext3/ext4 features (a journal, checksums, ...): read-only\n", dev_name);
        read_only = 1;
    }
    for (int i = 0; i < CACHE; i++) {
        cache[i].data = malloc(bs);
        if (!cache[i].data) return -ENOMEM;
    }
    uint8_t *d = gd(0, 0);
    if (!d || le32(d + 8) >= nblocks) return -EINVAL;
    if (!is_dir_ino(2)) return -EINVAL;
    // A filesystem id from the UUID (not 1-3: fsd's own)
    fsid = le32(sb + 104) ^ le32(sb + 108) ^ le32(sb + 112) ^ le32(sb + 116);
    if (fsid < 16) fsid += 16;
    if (!read_only) {
        uint32_t state = le16(sb + 58);
        if (!(state & 1) || (state & 2))
            fprintf(stderr, "ext2fs: %s: was not cleanly unmounted%s (run fsck.ext2 or e2fsck)\n", dev_name,
                    (state & 2) ? " and has errors" : "");
        // Not clean while mounted; one more mount
        put16(sb + 58, state & ~1u);
        put16(sb + 52, le16(sb + 52) + 1u);
        put32(sb + 44, (uint32_t)(now_ns() / 1000000000));
        if (write_super() < 0) return -EIO;
    }
    return 0;
}

int main(int argc, char **argv) {
    int a = 1;
    for (; a < argc && argv[a][0] == '-'; a++)
        if (!strcmp(argv[a], "-r")) read_only = 1;
    if (a != argc - 1) {
        fprintf(stderr, "usage: ext2fs [-r] DEVICE   (started by mount)\n");
        return 2;
    }
    dev_name = argv[a];
    rx_handle_t first = rx_take_startup_handle(RX_PA_USER0);
    if (!first) {
        fprintf(stderr, "ext2fs: no connection to serve (mount starts ext2fs)\n");
        return 2;
    }
    int r = rx_block_open(dev_name, &blk);
    rx_block_info_t info;
    if (r == 0) r = rx_block_info(blk, &info);
    if (r < 0) {
        fprintf(stderr, "ext2fs: %s: %s\n", dev_name, strerror(-r));
        return 1;
    }
    dev_bs = info.block_size;
    dev_blocks = info.block_count;
    if (info.flags & RX_BLOCK_FLAG_READONLY) read_only = 1;
    rx_handle_t vmo, dup;
    uint64_t va;
    if (rx_vmo_create(IOBUF, 0, &vmo) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, IOBUF, &va) != RX_OK ||
        rx_handle_duplicate(vmo, RX_RIGHT_SAME_RIGHTS, &dup) != RX_OK) {
        fprintf(stderr, "ext2fs: out of memory\n");
        return 1;
    }
    iobuf = (uint8_t *)va;
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_ATTACH_VMO;
    size_t got = 0;
    r = rx_block_call(blk, &q, 0, 0, dup, &vmoid, sizeof vmoid, &got);
    if (r < 0 || got != 4) {
        fprintf(stderr, "ext2fs: %s: cannot attach a buffer: %s\n", dev_name, strerror(r < 0 ? -r : EIO));
        return 1;
    }
    r = load();
    if (r < 0) {
        if (r == -EINVAL || r == -EIO) fprintf(stderr, "ext2fs: %s: not an ext2 filesystem%s\n", dev_name, r == -EIO ? " (read error)" : "");
        return 1;
    }
    struct ofd *o = new_ofd(2, O_RDONLY | O_DIRECTORY, "", 0);
    add_conn(first, o);

    static rx_wait_item_t items[MAX_CONNS];
    for (;;) {
        int n = nconns;
        if (n == 0 || shutting_down) {
            if (!read_only) {
                put16(sb + 58, le16(sb + 58) | 1);
                sb_dirty = 1;
            }
            flush();
            return 0;
        }
        for (int k = 0; k < n; k++) items[k] = (rx_wait_item_t){ conns[k].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        uint64_t deadline = write_pending ? rx_clock_get_monotonic() + 500000000ull : RX_TIME_INFINITE;
        rx_status_t ws = rx_object_wait_many(items, (uint64_t)n, deadline);
        if (ws == RX_ERR_TIMED_OUT) {
            flush();
            continue;
        }
        if (ws != RX_OK) continue;
        for (int k = n - 1; k >= 0; k--) {
            if (items[k].pending & RX_CHANNEL_READABLE) serve(k);
            else if (items[k].pending & RX_PEER_CLOSED) drop_conn(k);
        }
    }
}
