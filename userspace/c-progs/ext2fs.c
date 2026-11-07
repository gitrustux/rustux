// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ext2fs: the ext2, ext3 and ext4 filesystem server (steps 15 and 16).
// Serves the filesystem on a block device with fsd's file protocol
// (<rustux/fs.h>), read and write: block groups (flexible ones too, and
// groups whose bitmaps are not made yet), 32- and 64-byte group
// descriptors, inodes of 128 or 256 bytes (nanosecond times in the larger
// ones), extents and the older direct and indirect blocks, sparse files,
// directories (hashed ones are read as plain lists; adding a name to one
// turns it into a plain list, which e2fsck -D can index again), symbolic
// and hard links, owners, permission bits, metadata checksums (crc32c)
// and the older group checksums, more than 65000 subdirectories
// (dir_nlink), the orphan list.
//
//   ext2fs [-r] DEVICE      started by mount, with the server end of the
//                           root directory's connection as PA_USER0
//
// The journal (jbd2, in its inode): a filesystem that was not unmounted
// is first brought up to date from it (replay). Then every change of
// metadata goes through it, as a transaction of whole blocks (ordered
// mode: file data is written before the transaction that makes it part of
// a file): the journal copies, a flush, the commit block, a flush, then
// the blocks at home and a flush. Each transaction is checkpointed at
// once, so the journal never holds more than one, and nothing is ever
// revoked. Without a journal (ext2): the superblock says "not clean"
// while mounted, so e2fsck checks a filesystem that was not unmounted.
//
// Metadata reaches the disk before each reply, file data within half a
// second. Filesystems with features this server does not keep up to date
// (quotas, bigalloc, ...) are served read-only; those it cannot read
// (inline data, more than 2^32 blocks, ...) are refused.
// Format: Linux's Documentation/filesystems/ext4/ and "The Second
// Extended File System" (Dave Poirier).

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

#include "ext4lib.h"

#define MAX_CONNS 500
#define IOBUF (256u * 1024u)
#define CACHE 1024          // metadata blocks kept
#define PATHMAX 4096

#define S_IFMT_  0170000
#define S_IFDIR_ 0040000
#define S_IFREG_ 0100000
#define S_IFLNK_ 0120000
#define S_IFCHR_ 0020000

static const char *dev_name = "?";
static int read_only, dev_read_only;

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

static void dev_flush(void) {
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_FLUSH;
    rx_block_call(blk, &q, 0, 0, 0, 0, 0, 0);
}

// ---- the filesystem ----

static uint8_t sb[1024];
static int sb_dirty;
static uint32_t bs, spb;          // block size; device blocks per block
static uint32_t nblocks, ninodes, first_data, bpg, ipg, ngroups, isz, first_ino, gdt_block, desc_size;
static uint32_t feat_compat, feat_incompat, feat_ro;
static uint32_t fsid;
static struct x4 X;               // checksums
static int csum;                  // metadata_csum
static int uninit_ok;             // groups may be BLOCK_UNINIT / INODE_UNINIT (group checksums)
static uint32_t sb_home, sb_off;  // the block holding the superblock, and where in it
static uint8_t *sbblk;            // that block

static uint64_t blk_dev(uint32_t b) { return (uint64_t)b * spb; }

// ---- block cache (metadata and directories) ----

// What a cached block is, for its checksum
enum { K_NONE, K_GDT, K_BBM, K_IBM, K_ITAB, K_EXT, K_DIR, K_XATTR };

struct cblk {
    uint32_t block;     // 0: empty (block 0 is never cached)
    uint64_t used;
    int dirty;
    uint8_t kind;
    uint32_t aux;       // group (bitmaps), first inode (inode tables), owner's checksum seed
    uint8_t *data;
};

static struct cblk cache[CACHE];
static uint64_t cache_clock;
static int io_error;
static int write_pending;
static int journaling;            // metadata goes through the journal
static int in_flush;
static uint32_t max_dirty = CACHE - 32;

static int flush(void);
static uint8_t *gd(uint32_t g, struct cblk **cs);

static int nonzero(const uint8_t *p, uint32_t n) {
    for (uint32_t i = 0; i < n; i++) if (p[i]) return 1;
    return 0;
}

// Checksums of a block about to be written
static void seal(struct cblk *c) {
    if (!csum && !(X.gdt_csum && c->kind == K_GDT)) return;
    switch (c->kind) {
    case K_GDT: {
        uint32_t per = bs / desc_size, g0 = (c->block - gdt_block) * per;
        for (uint32_t k = 0; k < per && g0 + k < ngroups; k++) {
            uint8_t *d = c->data + k * desc_size;
            put16(d + 0x1E, x4_gd_csum(&X, g0 + k, d));
        }
        break;
    }
    case K_BBM:
    case K_IBM: {
        uint32_t cs = x4_bitmap_csum(&X, c->data, c->kind == K_BBM ? X.cpg : ipg);
        struct cblk *dc;
        uint8_t *d = gd(c->aux, &dc);
        if (!d) break;
        uint32_t lo = c->kind == K_BBM ? 0x18 : 0x1A, hi = c->kind == K_BBM ? 0x38 : 0x3A;
        put16(d + lo, cs);
        if (desc_size >= hi + 2) put16(d + hi, cs >> 16);
        dc->dirty = 1;
        break;
    }
    case K_ITAB:
        for (uint32_t k = 0; k < bs / isz; k++) {
            uint8_t *p = c->data + k * isz;
            if (nonzero(p, isz)) x4_inode_csum_set(&X, c->aux + k, p);
        }
        break;
    case K_EXT:
        if (le16(c->data) == XMAGIC && x4_ext_tail(c->data) + 4 <= bs) put32(c->data + x4_ext_tail(c->data), x4_ext_csum(c->aux, c->data));
        break;
    case K_DIR:
        if (x4_dir_has_tail(bs, c->data)) put32(c->data + bs - 4, x4_dir_csum(c->aux, bs, c->data));
        break;
    case K_XATTR:
        if (le32(c->data) == 0xEA020000u) put32(c->data + 0x10, x4_xattr_csum(&X, c->block, c->data));
        break;
    }
}

static int write_cblk(struct cblk *c) {
    seal(c);
    memcpy(iobuf, c->data, bs);
    int r = dev_rw(1, blk_dev(c->block), spb);
    if (r == 0) c->dirty = 0;
    else io_error = r;
    return r;
}

// A slot for another block: the least recently used. With a journal,
// changed blocks stay until their transaction is written (a full cache
// writes one first).
static struct cblk *pick_victim(void) {
    for (int attempt = 0;; attempt++) {
        struct cblk *v = 0;
        uint32_t dirty = 0;
        for (int i = 0; i < CACHE; i++) {
            if (cache[i].dirty) {
                dirty++;
                if (journaling) continue;
            }
            if (!v || cache[i].used < v->used) v = &cache[i];
        }
        if (journaling && !in_flush && attempt == 0 && (dirty >= max_dirty || !v)) {
            flush();
            continue;
        }
        if (!v) {
            io_error = -EIO;
            return 0;
        }
        if (v->dirty) {
            v->used = ++cache_clock;   // not picked again while it is written
            write_cblk(v);
        }
        v->block = 0;
        v->kind = K_NONE;
        v->dirty = 0;
        return v;
    }
}

static struct cblk *get_block(uint32_t block) {
    if (block == 0 || block >= nblocks) {
        io_error = -EIO;
        return 0;
    }
    for (int i = 0; i < CACHE; i++) {
        if (cache[i].block == block) {
            cache[i].used = ++cache_clock;
            return &cache[i];
        }
    }
    struct cblk *victim = pick_victim();
    if (!victim) return 0;
    if (dev_rw(0, blk_dev(block), spb) < 0) {
        io_error = -EIO;
        return 0;
    }
    memcpy(victim->data, iobuf, bs);
    victim->block = block;
    victim->used = ++cache_clock;
    return victim;
}

// A fresh (zeroed) cached block, without reading it
static struct cblk *new_block(uint32_t block) {
    struct cblk *c = 0;
    for (int i = 0; i < CACHE && !c; i++) if (cache[i].block == block) c = &cache[i];
    if (!c) {
        c = pick_victim();
        if (!c) return 0;
        c->block = block;
    }
    memset(c->data, 0, bs);
    c->kind = K_NONE;
    c->used = ++cache_clock;
    c->dirty = 1;
    return c;
}

// Freed blocks are no longer metadata: drop them (changed or not)
static void forget_range(uint32_t b, uint32_t n) {
    for (int i = 0; i < CACHE; i++) {
        if (cache[i].block >= b && cache[i].block - b < n) {
            cache[i].block = 0;
            cache[i].dirty = 0;
            cache[i].used = 0;
        }
    }
}

static void drop_cache(void) { forget_range(1, UINT32_MAX - 1); }

static void seal_super(void) {
    if (csum) put32(sb + 1020, x4_sb_csum(sb));
}

// The superblock: bytes 1024-2047 of the device, written in place
static int write_super(void) {
    seal_super();
    uint64_t first = 1024 / dev_bs, count = (1024 + dev_bs - 1) / dev_bs;
    uint32_t off = 1024 % dev_bs;
    if (count * dev_bs < off + 1024) count++;
    if (dev_rw(0, first, (uint32_t)count) < 0) return -EIO;
    memcpy(iobuf + off, sb, 1024);
    int r = dev_rw(1, first, (uint32_t)count);
    if (r == 0) sb_dirty = 0;
    return r;
}

// ---- writing runs of blocks ----

static uint32_t run_first, run_n;

static int run_flush(void) {
    if (!run_n) return 0;
    int r = dev_rw(1, blk_dev(run_first), run_n * spb);
    run_n = 0;
    if (r < 0) io_error = -EIO;
    return r;
}

// Write block `at` (with others next to it, at once)
static int run_put(uint32_t at, const uint8_t *data) {
    if (run_n && (at != run_first + run_n || (run_n + 1) * bs > IOBUF) && run_flush() < 0) return -EIO;
    if (!run_n) run_first = at;
    memcpy(iobuf + (size_t)run_n * bs, data, bs);
    run_n++;
    return 0;
}

// ---- the journal ----

static struct jbd jb;
static uint32_t j_seq;            // the next transaction's number
static uint32_t j_ino;
static uint8_t *jtmp;             // a descriptor, commit or escaped block

struct item { uint32_t home; const uint8_t *data; };

static int item_cmp(const void *a, const void *b) {
    uint32_t x = ((const struct item *)a)->home, y = ((const struct item *)b)->home;
    return x < y ? -1 : x > y;
}

// Tags per descriptor block
static uint32_t j_per_desc(void) {
    uint32_t space = bs - 12 - (jb.csum ? 4 : 0);
    return 1 + (space - jb.tag_bytes - 16) / jb.tag_bytes;
}

// The most blocks one transaction can hold
static uint32_t j_capacity(void) {
    uint32_t avail = jb.maxlen - jb.first - 1, per = j_per_desc();   // less the commit block
    return (uint32_t)((uint64_t)avail * per / (per + 1));
}

static void j_tag(uint8_t *t, uint32_t home, uint32_t flags, uint32_t cs) {
    if (jb.csum == 3) {
        putbe32(t, home);
        putbe32(t + 4, flags);
        putbe32(t + 8, 0);
        putbe32(t + 12, cs);
    } else {
        putbe32(t, home);
        putbe16(t + 4, jb.csum ? cs : 0);
        putbe16(t + 6, flags);
        if (jb.incompat & JBD_INCOMPAT_64BIT) putbe32(t + 8, 0);
    }
}

// One transaction: the journal superblock (pointing at it), descriptor and
// copied blocks, a flush, the commit block, a flush; then the blocks at
// home and a flush
static int j_transaction(struct item *it, uint32_t n) {
    uint32_t seq = j_seq;
    putbe32(jb.sbb + 0x18, seq);
    putbe32(jb.sbb + 0x1C, jb.first);
    jbd_sb_seal(&jb);
    if (run_put(jb.map[0], jb.sbb) < 0) return -EIO;
    uint32_t at = jb.first, per = j_per_desc();
    uint8_t sq[4];
    putbe32(sq, seq);
    for (uint32_t k = 0; k < n;) {
        uint32_t m = n - k < per ? n - k : per;
        uint8_t *d = jtmp;
        memset(d, 0, bs);
        putbe32(d, JBD_MAGIC);
        putbe32(d + 4, JBD_DESCRIPTOR);
        putbe32(d + 8, seq);
        uint32_t off = 12;
        for (uint32_t i = 0; i < m; i++) {
            const uint8_t *data = it[k + i].data;
            uint32_t flags = (i ? JBD_FLAG_SAME_UUID : 0) | (i == m - 1 ? JBD_FLAG_LAST_TAG : 0);
            uint32_t cs = 0;
            if (be32(data) == JBD_MAGIC) flags |= JBD_FLAG_ESCAPE;
            if (jb.csum) {
                cs = crc32c(jb.seed, sq, 4);
                if (flags & JBD_FLAG_ESCAPE) {
                    static const uint8_t z[4];
                    cs = crc32c(cs, z, 4);
                    cs = crc32c(cs, data + 4, bs - 4);
                } else {
                    cs = crc32c(cs, data, bs);
                }
                if (jb.csum == 2) cs &= 0xFFFF;
            }
            j_tag(d + off, it[k + i].home, flags, cs);
            off += jb.tag_bytes;
            if (!i) {
                memcpy(d + off, jb.sbb + 0x30, 16);
                off += 16;
            }
        }
        if (jb.csum) putbe32(d + bs - 4, crc32c(jb.seed, d, bs));
        if (run_put(jb.map[at++], d) < 0) return -EIO;
        for (uint32_t i = 0; i < m; i++) {
            const uint8_t *data = it[k + i].data;
            if (be32(data) == JBD_MAGIC) {
                memcpy(d, data, bs);
                memset(d, 0, 4);
                data = d;
            }
            if (run_put(jb.map[at++], data) < 0) return -EIO;
        }
        k += m;
    }
    if (run_flush() < 0) return -EIO;
    dev_flush();
    uint8_t *c = jtmp;
    memset(c, 0, bs);
    putbe32(c, JBD_MAGIC);
    putbe32(c + 4, JBD_COMMIT);
    putbe32(c + 8, seq);
    int64_t t = rx_clock_get_utc();
    putbe32(c + 0x30, (uint32_t)((uint64_t)(t / 1000000000) >> 32));
    putbe32(c + 0x34, (uint32_t)(t / 1000000000));
    putbe32(c + 0x38, (uint32_t)(t % 1000000000));
    if (jb.csum) putbe32(c + 16, crc32c(jb.seed, c, bs));
    if (run_put(jb.map[at], c) < 0 || run_flush() < 0) return -EIO;
    dev_flush();
    j_seq++;
    for (uint32_t k = 0; k < n; k++)
        if (run_put(it[k].home, it[k].data) < 0) return -EIO;
    if (run_flush() < 0) return -EIO;
    dev_flush();
    return 0;
}

// Write the changed blocks (and the superblock) through the journal
static int j_commit(void) {
    static struct item it[CACHE + 1];
    uint32_t n = 0;
    for (int i = 0; i < CACHE; i++) if (cache[i].dirty) it[n++] = (struct item){ cache[i].block, cache[i].data };
    if (sb_dirty) {
        seal_super();
        memcpy(sbblk + sb_off, sb, 1024);
        it[n++] = (struct item){ sb_home, sbblk };
    }
    if (!n) {
        dev_flush();   // file data written since
        return 0;
    }
    qsort(it, n, sizeof it[0], item_cmp);
    uint32_t cap = j_capacity();
    for (uint32_t k = 0; k < n; k += cap) {
        uint32_t m = n - k < cap ? n - k : cap;
        if (j_transaction(it + k, m) < 0) return -EIO;
    }
    for (int i = 0; i < CACHE; i++) cache[i].dirty = 0;
    sb_dirty = 0;
    return 0;
}

// Write everything out: checksums first (bitmaps change their group
// descriptors; descriptors last), then the blocks, in order
static int flush(void) {
    if (in_flush) return 0;
    in_flush = 1;
    write_pending = 0;
    if (csum || X.gdt_csum) {
        static const uint8_t order[] = { K_EXT, K_DIR, K_XATTR, K_BBM, K_IBM, K_ITAB, K_GDT };
        for (unsigned k = 0; k < sizeof order; k++)
            for (int i = 0; i < CACHE; i++)
                if (cache[i].dirty && cache[i].kind == order[k]) seal(&cache[i]);
    }
    if (journaling) {
        if (j_commit() < 0) io_error = -EIO;
    } else {
        static struct item it[CACHE];
        uint32_t n = 0;
        for (int i = 0; i < CACHE; i++) if (cache[i].dirty) it[n++] = (struct item){ cache[i].block, cache[i].data };
        qsort(it, n, sizeof it[0], item_cmp);
        int ok = 1;
        for (uint32_t k = 0; k < n && ok; k++) ok = run_put(it[k].home, it[k].data) == 0;
        if (ok && run_flush() == 0)
            for (int i = 0; i < CACHE; i++) cache[i].dirty = 0;
        if (sb_dirty && write_super() < 0) io_error = -EIO;
        dev_flush();
    }
    in_flush = 0;
    int r = io_error;
    io_error = 0;
    return r;
}

static int64_t now_ns(void) { return rx_clock_get_utc(); }

// ---- group descriptors ----

static uint8_t *gd(uint32_t g, struct cblk **cs) {
    struct cblk *c = get_block(gdt_block + g * desc_size / bs);
    if (!c) return 0;
    c->kind = K_GDT;
    if (cs) *cs = c;
    return c->data + (g * desc_size) % bs;
}

static void sb_add(int off, int delta) {
    put32(sb + off, le32(sb + off) + (uint32_t)delta);
    sb_dirty = 1;
}

// ---- inodes ----

static uint8_t *ino_ptr(uint32_t ino, struct cblk **cs) {
    if (ino == 0 || ino > ninodes) return 0;
    uint32_t g = (ino - 1) / ipg, i = (ino - 1) % ipg;
    uint8_t *d = gd(g, 0);
    if (!d) return 0;
    uint32_t table = le32(d + 8);
    struct cblk *b = get_block(table + (uint32_t)((uint64_t)i * isz / bs));
    if (!b) return 0;
    b->kind = K_ITAB;
    b->aux = ino - i % (bs / isz);
    if (cs) *cs = b;
    return b->data + ((uint64_t)i * isz) % bs;
}

// The seed of an inode's own checksums
static uint32_t iseed(uint32_t ino) {
    if (!csum) return 0;
    uint8_t *p = ino_ptr(ino, 0);
    return p ? x4_inode_seed(&X, ino, le32(p + 100)) : 0;
}

static uint32_t i_mode(uint32_t ino) { uint8_t *p = ino_ptr(ino, 0); return p ? le16(p) : 0; }
static uint32_t i_flags(uint32_t ino) { uint8_t *p = ino_ptr(ino, 0); return p ? le32(p + 32) : 0; }
// ext4 keeps the size's high half for every file; ext2 only for regular files
static int size_hi(const uint8_t *p) { return (le16(p) & S_IFMT_) == S_IFREG_ || (feat_incompat & INCOMPAT_EXTENTS); }
static uint64_t i_size(uint32_t ino) {
    uint8_t *p = ino_ptr(ino, 0);
    if (!p) return 0;
    uint64_t s = le32(p + 4);
    if (size_hi(p)) s |= (uint64_t)le32(p + 108) << 32;
    return s;
}
static void set_size_field(uint32_t ino, uint64_t size) {
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return;
    put32(p + 4, (uint32_t)size);
    if (size_hi(p)) put32(p + 108, (uint32_t)(size >> 32));
    if ((le16(p) & S_IFMT_) == S_IFREG_ && size >= 0x80000000ull && !(feat_ro & RO_LARGE_FILE)) {
        feat_ro |= RO_LARGE_FILE;
        put32(sb + 100, feat_ro);
        sb_dirty = 1;
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
    if (le32(p + 32) & HUGE_FILE_FL) sectors /= bs / 512;   // counted in blocks
    uint64_t v = le32(p + 28) | (uint64_t)le16(p + 116) << 32;
    v += (uint64_t)sectors;
    put32(p + 28, (uint32_t)v);
    put16(p + 116, (uint32_t)(v >> 32));
    c->dirty = 1;
}

// ---- bitmaps ----

static uint32_t in_group(uint32_t g) { return g == ngroups - 1 ? nblocks - first_data - g * bpg : bpg; }

static void set_bits(uint8_t *m, uint32_t from, uint32_t to) {
    for (uint32_t i = from; i < to; i++) m[i / 8] |= (uint8_t)(1u << (i % 8));
}

// A group's block bitmap as Linux makes it for a BLOCK_UNINIT group: its
// superblock copy and descriptor blocks, its own bitmaps and inode table
// (where they are in it), and the bits past its end
static void init_block_bitmap(uint32_t g, uint8_t *m, const uint8_t *d) {
    memset(m, 0, bs);
    uint32_t start = first_data + g * bpg, n = in_group(g);
    if (x4_has_super(sb, g)) {
        uint32_t gdt_blocks = (ngroups * desc_size + bs - 1) / bs;
        set_bits(m, 0, 1 + gdt_blocks + le16(sb + 206));
    }
    uint32_t tb = (ipg * isz + bs - 1) / bs, own[3] = { le32(d), le32(d + 4), le32(d + 8) };
    for (int k = 0; k < 3; k++) {
        uint32_t len = k == 2 ? tb : 1;
        for (uint32_t b = own[k]; b < own[k] + len; b++)
            if (b >= start && b - start < n) set_bits(m, b - start, b - start + 1);
    }
    set_bits(m, n, bs * 8);
}

// A group's bitmap (blocks, or inodes), made first if the group has none
// yet (then it no longer is "uninit")
static struct cblk *group_bitmap(uint32_t g, int inodes) {
    struct cblk *dc;
    uint8_t *d = gd(g, &dc);
    if (!d) return 0;
    uint32_t flag = inodes ? BG_INODE_UNINIT : BG_BLOCK_UNINIT, at = le32(d + (inodes ? 4 : 0));
    struct cblk *b;
    if (uninit_ok && (le16(d + 18) & flag)) {
        if (read_only) return 0;
        b = new_block(at);
        if (!b) return 0;
        d = gd(g, &dc);
        if (inodes) set_bits(b->data, ipg, bs * 8);
        else init_block_bitmap(g, b->data, d);
        put16(d + 18, le16(d + 18) & ~flag);
        dc->dirty = 1;
    } else {
        b = get_block(at);
        if (!b) return 0;
    }
    b->kind = inodes ? K_IBM : K_BBM;
    b->aux = g;
    return b;
}

// ---- allocation ----

// A free block, near `goal` if possible; 0 when the disk is full
static uint32_t alloc_block(uint32_t goal) {
    if (le32(sb + 12) == 0) return 0;
    if (goal < first_data || goal >= nblocks) goal = first_data;
    uint32_t g0 = (goal - first_data) / bpg;
    for (uint32_t k = 0; k < ngroups; k++) {
        uint32_t g = (g0 + k) % ngroups;
        uint8_t *d = gd(g, 0);
        if (!d || le16(d + 12) == 0) continue;
        struct cblk *bm = group_bitmap(g, 0);
        if (!bm) return 0;
        uint32_t n = in_group(g);
        uint32_t start = k == 0 ? (goal - first_data) % bpg : 0;
        for (uint32_t step = 0; step < n;) {
            uint32_t i = (start + step) % n;
            if (i % 8 == 0 && i + 8 <= n && step + 8 <= n && bm->data[i / 8] == 0xFF) {
                step += 8;
                continue;
            }
            step++;
            if (bm->data[i / 8] & (1u << (i % 8))) continue;
            bm->data[i / 8] |= (uint8_t)(1u << (i % 8));
            bm->dirty = 1;
            struct cblk *dc;
            d = gd(g, &dc);
            put16(d + 12, le16(d + 12) - 1u);
            dc->dirty = 1;
            sb_add(12, -1);
            return first_data + g * bpg + i;
        }
    }
    return 0;
}

static void free_range(uint32_t b, uint32_t n) {
    uint32_t b0 = b, n0 = n;
    while (n) {
        if (b < first_data || b >= nblocks) break;
        uint32_t g = (b - first_data) / bpg, i = (b - first_data) % bpg;
        uint32_t m = bpg - i < n ? bpg - i : n;
        struct cblk *bm = group_bitmap(g, 0);
        if (!bm) break;
        uint32_t freed = 0;
        for (uint32_t k = i; k < i + m; k++) {
            if (!(bm->data[k / 8] & (1u << (k % 8)))) continue;
            bm->data[k / 8] &= (uint8_t)~(1u << (k % 8));
            freed++;
        }
        if (freed) {
            bm->dirty = 1;
            struct cblk *dc;
            uint8_t *d = gd(g, &dc);
            if (d) {
                put16(d + 12, le16(d + 12) + freed);
                dc->dirty = 1;
            }
            sb_add(12, (int)freed);
        }
        b += m;
        n -= m;
    }
    forget_range(b0, n0);
}

static void free_block(uint32_t b) { free_range(b, 1); }

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
        uint8_t *d = gd(g, 0);
        if (!d || le16(d + 14) == 0) continue;
        struct cblk *bm = group_bitmap(g, 1);
        if (!bm) return 0;
        for (uint32_t i = 0; i < ipg; i++) {
            uint32_t ino = g * ipg + i + 1;
            if (ino < first_ino) continue;
            if (bm->data[i / 8] & (1u << (i % 8))) continue;
            bm->data[i / 8] |= (uint8_t)(1u << (i % 8));
            bm->dirty = 1;
            // Linux makes the block bitmap of a group it puts inodes in
            if (uninit_ok) group_bitmap(g, 0);
            struct cblk *dc;
            d = gd(g, &dc);
            put16(d + 14, le16(d + 14) - 1u);
            if (dir) put16(d + 16, le16(d + 16) + 1u);
            if (uninit_ok) {
                // Inodes from the end of the table on were never used
                uint32_t unused = le16(d + 0x1C);
                if (i >= ipg - unused) put16(d + 0x1C, ipg - i - 1);
            }
            dc->dirty = 1;
            sb_add(16, -1);
            return ino;
        }
    }
    return 0;
}

static void free_inode_bit(uint32_t ino, int dir) {
    uint32_t g = (ino - 1) / ipg, i = (ino - 1) % ipg;
    struct cblk *bm = group_bitmap(g, 1);
    if (!bm || !(bm->data[i / 8] & (1u << (i % 8)))) return;
    bm->data[i / 8] &= (uint8_t)~(1u << (i % 8));
    bm->dirty = 1;
    struct cblk *dc;
    uint8_t *d = gd(g, &dc);
    if (!d) return;
    put16(d + 14, le16(d + 14) + 1u);
    if (dir) put16(d + 16, le16(d + 16) - 1u);
    dc->dirty = 1;
    sb_add(16, 1);
}

static uint32_t ino_goal(uint32_t ino) { return first_data + ((ino - 1) / ipg) * bpg; }

// ---- extents ----
//
// A tree: the root (up to 4 entries) in the inode's block pointers, other
// nodes in blocks. Index nodes: (first logical block, child node); leaves:
// (first logical block, length, first physical block); an unwritten
// extent (fallocate) reads as zeros.

#define XMAXD 6

struct xlev { uint32_t blk; int i; };   // blk 0: the root; i: the entry taken (-1: before the first)

static uint8_t *xnode(uint32_t ino, uint32_t blk, struct cblk **cs) {
    if (!blk) {
        uint8_t *p = ino_ptr(ino, cs);
        return p ? p + 40 : 0;
    }
    uint32_t seed = iseed(ino);
    struct cblk *c = get_block(blk);
    if (!c) return 0;
    c->kind = K_EXT;
    c->aux = seed;
    *cs = c;
    return c->data;
}

static uint32_t x_len(const uint8_t *e) { uint32_t l = le16(e + 4); return l > 32768 ? l - 32768 : l; }
static int x_unwritten(const uint8_t *e) { return le16(e + 4) > 32768; }

// The path from the root to the leaf where logical block lb is (or would be)
static int x_path(uint32_t ino, uint32_t lb, struct xlev *pt, int *depth) {
    uint32_t blk = 0;
    for (int lv = 0; lv < XMAXD; lv++) {
        struct cblk *c;
        uint8_t *h = xnode(ino, blk, &c);
        if (!h || le16(h) != XMAGIC) return -EIO;
        uint32_t n = le16(h + 2), d = le16(h + 6);
        if (lv == 0) {
            if (d >= XMAXD) return -EIO;
            *depth = (int)d;
        } else if (d != (uint32_t)(*depth - lv)) {
            return -EIO;
        }
        if (n > le16(h + 4) || 12u + 12u * le16(h + 4) > (blk ? bs : 60u)) return -EIO;
        int i = -1;
        for (uint32_t k = 0; k < n && le32(h + 12 + 12 * k) <= lb; k++) i = (int)k;
        pt[lv].blk = blk;
        if (d == 0) {
            pt[lv].i = i;
            return 0;
        }
        if (n == 0) return -EIO;
        if (i < 0) i = 0;
        pt[lv].i = i;
        uint8_t *e = h + 12 + 12 * i;
        if (le16(e + 8)) return -EIO;   // beyond 32 bits
        blk = le32(e + 4);
    }
    return -EIO;
}

// Logical block lb: its physical block (0: a hole), -1 on error.
// *unwritten for an unwritten extent; *hint: the block that would follow
// the extent before it
static int64_t x_lookup(uint32_t ino, uint32_t lb, int *unwritten, uint32_t *hint) {
    struct xlev pt[XMAXD];
    int d;
    *unwritten = 0;
    if (hint) *hint = 0;
    if (x_path(ino, lb, pt, &d) < 0) return -1;
    struct cblk *c;
    uint8_t *h = xnode(ino, pt[d].blk, &c);
    if (!h || pt[d].i < 0) return 0;
    uint8_t *e = h + 12 + 12 * pt[d].i;
    uint32_t eb = le32(e), len = x_len(e);
    if (le16(e + 6)) return -1;
    if (hint) *hint = le32(e + 8) + (lb - eb);
    if ((uint64_t)lb >= (uint64_t)eb + len) return 0;
    *unwritten = x_unwritten(e);
    return (int64_t)le32(e + 8) + (lb - eb);
}

// Lower the keys on the path to lb (a new first entry of the leftmost leaf)
static void x_fix_keys(uint32_t ino, struct xlev *pt, int d, uint32_t lb) {
    for (int lv = 0; lv < d; lv++) {
        struct cblk *c;
        uint8_t *h = xnode(ino, pt[lv].blk, &c);
        if (!h) return;
        uint8_t *e = h + 12 + 12 * pt[lv].i;
        if (le32(e) > lb) {
            put32(e, lb);
            c->dirty = 1;
        }
    }
}

// Make room in the full node at level lv of the path: a full root moves
// into a new block below it; another node splits in two (or, for the last
// leaf when adding past its end, gets an empty one after it). The caller
// looks its path up again.
static int x_make_room(uint32_t ino, struct xlev *pt, int d, int lv, uint32_t lb) {
    struct cblk *c, *pc = 0;
    uint8_t *h = xnode(ino, pt[lv].blk, &c), *ph;
    if (!h) return -EIO;
    uint32_t n = le16(h + 2), cap = (bs - 12) / 12;
    if (n < le16(h + 4)) return 0;
    if (lv > 0) {
        ph = xnode(ino, pt[lv - 1].blk, &pc);
        if (!ph) return -EIO;
        if (le16(ph + 2) >= le16(ph + 4)) return x_make_room(ino, pt, d, lv - 1, lb);
    }
    uint32_t nb = alloc_block(pt[lv].blk ? pt[lv].blk + 1 : ino_goal(ino));
    if (!nb) return -ENOSPC;
    add_iblocks(ino, bs / 512);
    uint32_t seed = iseed(ino);
    struct cblk *nc = new_block(nb);
    if (!nc) return -EIO;
    nc->kind = K_EXT;
    nc->aux = seed;
    h = xnode(ino, pt[lv].blk, &c);
    if (lv == 0) {
        memcpy(nc->data, h, 12 + 12 * n);
        put16(nc->data + 4, cap);
        uint8_t *e = h + 12;
        put32(e, n ? le32(nc->data + 12) : lb);
        put32(e + 4, nb);
        put16(e + 8, 0);
        put16(e + 10, 0);
        put16(h + 2, 1);
        put16(h + 6, le16(h + 6) + 1u);
        c->dirty = 1;
        return 0;
    }
    int append = lv == d;
    for (int k = 0; k <= lv && append; k++) {
        struct cblk *kc;
        uint8_t *kh = xnode(ino, pt[k].blk, &kc);
        append = kh && pt[k].i == (int)le16(kh + 2) - 1;
    }
    h = xnode(ino, pt[lv].blk, &c);
    uint32_t move = append ? 0 : n / 2;
    memcpy(nc->data, h, 12);
    put16(nc->data + 2, move);
    put16(nc->data + 4, cap);
    memcpy(nc->data + 12, h + 12 + 12 * (n - move), 12 * move);
    put16(h + 2, n - move);
    c->dirty = 1;
    uint32_t key = move ? le32(nc->data + 12) : lb;
    ph = xnode(ino, pt[lv - 1].blk, &pc);
    uint32_t pn = le16(ph + 2), at = (uint32_t)pt[lv - 1].i + 1;
    memmove(ph + 12 + 12 * (at + 1), ph + 12 + 12 * at, 12 * (pn - at));
    uint8_t *e = ph + 12 + 12 * at;
    put32(e, key);
    put32(e + 4, nb);
    put16(e + 8, 0);
    put16(e + 10, 0);
    put16(ph + 2, pn + 1);
    pc->dirty = 1;
    return 0;
}

// Map `count` logical blocks from lb (a hole) to physical blocks from pb
static int x_insert(uint32_t ino, uint32_t lb, uint32_t pb, uint32_t count, int unwritten) {
    uint32_t maxlen = unwritten ? 32767 : 32768;
    for (int tries = 0; tries < 20; tries++) {
        struct xlev pt[XMAXD];
        int d;
        if (x_path(ino, lb, pt, &d) < 0) return -EIO;
        struct cblk *c;
        uint8_t *h = xnode(ino, pt[d].blk, &c);
        if (!h) return -EIO;
        uint32_t n = le16(h + 2);
        int i = pt[d].i;
        if (i >= 0) {
            // Grow the extent before it
            uint8_t *e = h + 12 + 12 * i;
            uint32_t len = x_len(e);
            if (x_unwritten(e) == unwritten && le32(e) + len == lb && le32(e + 8) + len == pb && len + count <= maxlen && !le16(e + 6)) {
                put16(e + 4, le16(e + 4) + count);
                c->dirty = 1;
                return 0;
            }
        }
        if ((uint32_t)(i + 1) < n) {
            // Or the one after it
            uint8_t *e = h + 12 + 12 * (i + 1);
            uint32_t len = x_len(e);
            if (x_unwritten(e) == unwritten && le32(e) == lb + count && le32(e + 8) == pb + count && len + count <= maxlen && !le16(e + 6)) {
                put32(e, lb);
                put32(e + 8, pb);
                put16(e + 4, le16(e + 4) + count);
                c->dirty = 1;
                if (i + 1 == 0) x_fix_keys(ino, pt, d, lb);
                return 0;
            }
        }
        if (n < le16(h + 4)) {
            uint8_t *e = h + 12 + 12 * (i + 1);
            memmove(e + 12, e, 12 * (n - (uint32_t)(i + 1)));
            put32(e, lb);
            put16(e + 4, (unwritten ? 32768 : 0) + count);
            put16(e + 6, 0);
            put32(e + 8, pb);
            put16(h + 2, n + 1);
            c->dirty = 1;
            if (i + 1 == 0) x_fix_keys(ino, pt, d, lb);
            return 0;
        }
        int r = x_make_room(ino, pt, d, d, lb);
        if (r < 0) return r;
    }
    return -EIO;
}

// Take block lb out of the unwritten extent it is in (which shrinks or splits)
static int x_punch1(uint32_t ino, uint32_t lb) {
    struct xlev pt[XMAXD];
    int d;
    if (x_path(ino, lb, pt, &d) < 0 || pt[d].i < 0) return -EIO;
    struct cblk *c;
    uint8_t *h = xnode(ino, pt[d].blk, &c);
    if (!h) return -EIO;
    uint8_t *e = h + 12 + 12 * pt[d].i;
    uint32_t eb = le32(e), len = x_len(e), start = le32(e + 8), un = le16(e + 4) - len;
    c->dirty = 1;
    if (len == 1) {
        uint32_t n = le16(h + 2);
        memmove(e, e + 12, 12 * (n - (uint32_t)pt[d].i - 1));
        put16(h + 2, n - 1);
    } else if (lb == eb) {
        put32(e, eb + 1);
        put32(e + 8, start + 1);
        put16(e + 4, un + len - 1);
    } else if (lb == eb + len - 1) {
        put16(e + 4, un + len - 1);
    } else {
        uint32_t before = lb - eb;
        put16(e + 4, un + before);
        return x_insert(ino, lb + 1, start + before + 1, len - before - 1, 1);
    }
    return 0;
}

static int64_t x_map(uint32_t ino, uint64_t lb64, int alloc, uint32_t goal, int *fresh) {
    if (lb64 >= 0xFFFFFFFFull) return alloc ? -1 : 0;
    uint32_t lb = (uint32_t)lb64, hint;
    int unw;
    int64_t pb = x_lookup(ino, lb, &unw, &hint);
    if (pb < 0) return -1;
    if (pb && !unw) return pb;
    if (!alloc) return 0;   // holes and unwritten extents read as zeros
    if (read_only) return -1;
    if (pb) {
        if (x_punch1(ino, lb) < 0 || x_insert(ino, lb, (uint32_t)pb, 1, 0) < 0) return -1;
        if (fresh) *fresh = 1;
        return pb;
    }
    uint32_t nb = alloc_block(goal ? goal : hint ? hint : ino_goal(ino));
    if (!nb) return -1;
    add_iblocks(ino, bs / 512);
    if (x_insert(ino, lb, nb, 1, 0) < 0) {
        free_block(nb);
        add_iblocks(ino, -(int64_t)(bs / 512));
        return -1;
    }
    if (fresh) *fresh = 1;
    return nb;
}

// Free the blocks from logical block `from` on below node blk; 1 if the
// node is empty then
static int x_trunc_node(uint32_t ino, uint32_t blk, uint32_t from, int guard) {
    struct cblk *c;
    uint8_t *h = xnode(ino, blk, &c);
    if (!h || le16(h) != XMAGIC || guard > XMAXD) return 0;
    uint32_t d = le16(h + 6);
    for (int k = (int)le16(h + 2) - 1; k >= 0; k--) {
        h = xnode(ino, blk, &c);
        if (!h) return 0;
        uint8_t *e = h + 12 + 12 * k;
        uint32_t key = le32(e);
        if (d == 0) {
            uint32_t len = x_len(e), start = le32(e + 8), un = le16(e + 4) - len;
            if (key + len <= from) break;
            uint32_t keep = key >= from ? 0 : from - key;
            free_range(start + keep, len - keep);
            add_iblocks(ino, -(int64_t)(len - keep) * (bs / 512));
            h = xnode(ino, blk, &c);
            e = h + 12 + 12 * k;
            if (keep) {
                put16(e + 4, un + keep);
            } else {
                uint32_t n = le16(h + 2);
                memmove(e, e + 12, 12 * (n - (uint32_t)k - 1));
                put16(h + 2, n - 1);
            }
            c->dirty = 1;
            if (keep) break;
        } else {
            uint32_t child = le32(e + 4);
            if (x_trunc_node(ino, child, from, guard + 1)) {
                free_block(child);
                add_iblocks(ino, -(int64_t)(bs / 512));
                h = xnode(ino, blk, &c);
                e = h + 12 + 12 * k;
                uint32_t n = le16(h + 2);
                memmove(e, e + 12, 12 * (n - (uint32_t)k - 1));
                put16(h + 2, n - 1);
                c->dirty = 1;
            }
            if (key <= from) break;
        }
    }
    h = xnode(ino, blk, &c);
    return h && le16(h + 2) == 0;
}

static void x_free_from(uint32_t ino, uint64_t from) {
    if (from >= 0xFFFFFFFFull) return;
    if (x_trunc_node(ino, 0, (uint32_t)from, 0)) {
        // Nothing left: a leaf again
        struct cblk *c;
        uint8_t *h = xnode(ino, 0, &c);
        if (h && le16(h + 6)) {
            put16(h + 6, 0);
            c->dirty = 1;
        }
    }
}

static void x_init_root(uint8_t *p) {
    put32(p + 32, le32(p + 32) | EXTENTS_FL);
    memset(p + 40, 0, 60);
    put16(p + 40, XMAGIC);
    put16(p + 44, 4);
}

// ---- block map ----

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
    if (le32(p + 32) & EXTENTS_FL) return x_map(ino, lb, alloc, goal, fresh);
    if (!goal) goal = ino_goal(ino);
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
    if (le32(p + 32) & EXTENTS_FL) {
        x_free_from(ino, from);
        return;
    }
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

// The largest file: 2^32 blocks with extents, the indirect blocks' reach without
static uint64_t max_size(uint32_t ino) {
    if (i_flags(ino) & EXTENTS_FL) return 0xFFFFFFFFull * bs;
    uint64_t per = per_block();
    return (12 + per + per * per + per * per * per) * bs;
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
    if (size > max_size(ino)) return -EFBIG;
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

// A block of directory `dir` (its checksum tail, if any, is kept up to date)
static struct cblk *dir_block(uint32_t dir, uint32_t pb) {
    uint32_t seed = iseed(dir);
    struct cblk *c = get_block(pb);
    if (c) {
        c->kind = K_DIR;
        c->aux = seed;
    }
    return c;
}

static struct cblk *dir_new_block(uint32_t dir, uint32_t pb) {
    uint32_t seed = iseed(dir);
    struct cblk *c = new_block(pb);
    if (c) {
        c->kind = K_DIR;
        c->aux = seed;
        if (csum) x4_dir_init_tail(bs, c->data);
    }
    return c;
}

// Where the entries of a block end (before the checksum tail)
static uint32_t dir_end(const struct cblk *c) { return x4_dir_has_tail(bs, c->data) ? bs - 12 : bs; }

// The next entry at or after *pos: 1 (d filled, *pos past it), 0 at the end
static int dir_next(uint32_t dir, uint32_t *pos, struct dent *d) {
    uint64_t size = i_size(dir);
    while (*pos < size) {
        uint32_t lb = *pos / bs, in = *pos % bs;
        int64_t pb = bmap(dir, lb, 0, 0, 0);
        if (pb <= 0) { *pos = (lb + 1) * bs; continue; }
        struct cblk *c = dir_block(dir, (uint32_t)pb);
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

// A hashed directory becomes a plain list: the index root (block 0) keeps
// "." and "..", the other index nodes become empty blocks; the leaves stay
static int dir_unindex(uint32_t dir) {
    struct cblk *ic;
    uint8_t *p = ino_ptr(dir, &ic);
    if (!p || !(le32(p + 32) & INDEX_FL)) return 0;
    uint64_t size = i_size(dir);
    for (uint32_t lb = 0; (uint64_t)lb * bs < size; lb++) {
        int64_t pb = bmap(dir, lb, 0, 0, 0);
        if (pb <= 0) continue;
        struct cblk *c = dir_block(dir, (uint32_t)pb);
        if (!c) return -EIO;
        uint8_t *b = c->data;
        uint32_t end = csum ? bs - 12 : bs;
        if (lb == 0) {
            if (le16(b + 4) != 12 || le16(b + 12 + 4) < 12) return -EIO;
            memset(b + 24, 0, bs - 24);
            put16(b + 16, end - 12);
        } else if (le32(b) == 0 && le16(b + 4) == bs && b[6] == 0) {
            memset(b, 0, bs);
            put16(b + 4, end);
        } else {
            continue;
        }
        if (csum) x4_dir_init_tail(bs, b);
        c->dirty = 1;
    }
    p = ino_ptr(dir, &ic);
    put32(p + 32, le32(p + 32) & ~(uint32_t)INDEX_FL);
    ic->dirty = 1;
    return 0;
}

static uint32_t rec_need(uint32_t nl) { return (8 + nl + 3) & ~3u; }

static void put_entry(uint8_t *n, uint32_t rec, const char *name, size_t len, uint32_t ino, uint8_t type) {
    put32(n, ino);
    put16(n + 4, rec);
    if (feat_incompat & INCOMPAT_FILETYPE) { n[6] = (uint8_t)len; n[7] = type; }
    else put16(n + 6, (uint32_t)len);
    memcpy(n + 8, name, len);
}

static int dir_add(uint32_t dir, const char *name, size_t len, uint32_t ino, uint8_t type) {
    int r = dir_unindex(dir);
    if (r < 0) return r;
    uint32_t need = rec_need((uint32_t)len);
    uint64_t size = i_size(dir);
    for (uint32_t lb = 0; (uint64_t)lb * bs < size; lb++) {
        int64_t pb = bmap(dir, lb, 0, 0, 0);
        if (pb <= 0) continue;
        struct cblk *c = dir_block(dir, (uint32_t)pb);
        if (!c) return -EIO;
        uint32_t end = dir_end(c);
        for (uint32_t in = 0; in < end;) {
            uint8_t *e = c->data + in;
            uint32_t rec = le16(e + 4);
            if (rec < 8 || in + rec > end) break;
            uint32_t used = le32(e) ? rec_need(name_len_of(e)) : 0;
            if (rec - used >= need) {
                uint8_t *n = e;
                if (used) {
                    put16(e + 4, used);
                    n = e + used;
                }
                put_entry(n, rec - used, name, len, ino, type);
                c->dirty = 1;
                touch(dir, 1, 1);
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
    struct cblk *c = dir_new_block(dir, (uint32_t)pb);
    if (!c) return -EIO;
    put_entry(c->data, csum ? bs - 12 : bs, name, len, ino, type);
    set_size_field(dir, (uint64_t)(lb + 1) * bs);
    touch(dir, 1, 1);
    return 0;
}

// Remove the entry at byte offset `off`: the one before it in the block
// takes its space (or, first in the block, it is marked unused). A hashed
// directory keeps its index (the names left are where it says).
static int dir_remove(uint32_t dir, uint32_t off) {
    uint32_t lb = off / bs, at = off % bs;
    int64_t pb = bmap(dir, lb, 0, 0, 0);
    if (pb <= 0) return -EIO;
    struct cblk *c = dir_block(dir, (uint32_t)pb);
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
    touch(dir, 1, 1);
    return 0;
}

// Point the entry at `off` to another inode
static int dir_retarget(uint32_t dir, uint32_t off, uint32_t ino, uint8_t type) {
    int64_t pb = bmap(dir, off / bs, 0, 0, 0);
    if (pb <= 0) return -EIO;
    struct cblk *c = dir_block(dir, (uint32_t)pb);
    if (!c) return -EIO;
    uint8_t *e = c->data + off % bs;
    put32(e, ino);
    if (feat_incompat & INCOMPAT_FILETYPE) e[7] = type;
    c->dirty = 1;
    touch(dir, 1, 1);
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

// A directory's link count: 2 + its subdirectories; past 65000 (with
// dir_nlink) it is 1, which means "many"
static int dir_link_inc(uint32_t dir) {
    uint32_t l = i_links(dir);
    if (l == 1 && (feat_ro & RO_DIR_NLINK)) return 0;
    if (l >= 65000) {
        if (!(feat_ro & RO_DIR_NLINK)) return -EMLINK;
        set_links(dir, 1);
        return 0;
    }
    set_links(dir, l + 1);
    return 0;
}

static int dir_link_ok(uint32_t dir) { return i_links(dir) < 65000 || (feat_ro & RO_DIR_NLINK); }

static void dir_link_dec(uint32_t dir) {
    uint32_t l = i_links(dir);
    if (l > 2) set_links(dir, l - 1);
}

// ---- open inodes ----

struct onode {
    uint32_t ino;
    int refs;
    int orphan;          // on the orphan list (no names left, still open)
    struct onode *next;
};

static struct onode *onodes;

static struct onode *onode_of(uint32_t ino) {
    for (struct onode *n = onodes; n; n = n->next) if (n->ino == ino) return n;
    return 0;
}

static void ino_ref(uint32_t ino) {
    struct onode *n = onode_of(ino);
    if (n) { n->refs++; return; }
    n = calloc(1, sizeof *n);
    if (!n) return;
    n->ino = ino;
    n->refs = 1;
    n->next = onodes;
    onodes = n;
}

// The orphan list: inodes without names that are still open (or, after a
// crash, half truncated), so that the next mount (or e2fsck) frees them.
// The superblock names the first; each inode's dtime field the next.
static void orphan_add(uint32_t ino) {
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return;
    put32(p + 20, le32(sb + 232));
    c->dirty = 1;
    put32(sb + 232, ino);
    sb_dirty = 1;
}

static void orphan_remove(uint32_t ino) {
    struct cblk *c;
    uint8_t *p = ino_ptr(ino, &c);
    if (!p) return;
    uint32_t next = le32(p + 20);
    if (le32(sb + 232) == ino) {
        put32(sb + 232, next);
        sb_dirty = 1;
        return;
    }
    uint32_t cur = le32(sb + 232);
    for (int guard = 0; cur && guard < 100000; guard++) {
        struct cblk *cc;
        uint8_t *q = ino_ptr(cur, &cc);
        if (!q) return;
        if (le32(q + 20) == ino) {
            put32(q + 20, next);
            cc->dirty = 1;
            return;
        }
        cur = le32(q + 20);
    }
}

// The extended attribute block goes with its last inode
static void xattr_release(uint32_t ino) {
    struct cblk *ic;
    uint8_t *p = ino_ptr(ino, &ic);
    if (!p) return;
    uint32_t b = le32(p + 104);
    if (!b || le16(p + 118)) return;
    put32(p + 104, 0);
    ic->dirty = 1;
    add_iblocks(ino, -(int64_t)(bs / 512));
    struct cblk *c = get_block(b);
    if (!c || le32(c->data) != 0xEA020000u) return;
    uint32_t refs = le32(c->data + 4);
    if (refs > 1) {
        put32(c->data + 4, refs - 1);
        c->kind = K_XATTR;
        c->dirty = 1;
    } else {
        free_block(b);
    }
}

// Fast symbolic links keep their target in the block pointers
static int is_fast_link(const uint8_t *p) {
    if ((le16(p) & S_IFMT_) != S_IFLNK_ || (le32(p + 32) & EXTENTS_FL)) return 0;
    uint64_t blocks = le32(p + 28);
    return blocks == 0 || (le32(p + 104) && blocks == bs / 512);
}

// An inode's blocks and its bitmap bit go
static void release_inode(uint32_t ino) {
    uint8_t *p = ino_ptr(ino, 0);
    if (!p) return;
    uint32_t mode = le16(p), type = mode & S_IFMT_;
    int fast_link = is_fast_link(p);
    xattr_release(ino);
    if ((type == S_IFREG_ || type == S_IFDIR_ || type == S_IFLNK_) && !fast_link) free_from(ino, 0);
    struct cblk *c;
    p = ino_ptr(ino, &c);
    if (!p) return;
    put32(p + 20, (uint32_t)(now_ns() / 1000000000));   // dtime
    if (fast_link) memset(p + 40, 0, 60);
    put32(p + 4, 0);
    put32(p + 108, 0);
    c->dirty = 1;
    free_inode_bit(ino, type == S_IFDIR_);
}

// An inode without names or openers goes; without names but open, it
// waits on the orphan list
static void maybe_free_inode(uint32_t ino) {
    if (i_links(ino) || read_only) return;
    if (!i_mode(ino)) return;
    struct onode *n = onode_of(ino);
    if (n && n->refs) {
        if (!n->orphan) {
            orphan_add(ino);
            n->orphan = 1;
        }
        return;
    }
    release_inode(ino);
}

static void ino_unref(uint32_t ino) {
    for (struct onode **pp = &onodes; *pp; pp = &(*pp)->next) {
        struct onode *n = *pp;
        if (n->ino != ino) continue;
        if (--n->refs == 0) {
            int orphan = n->orphan;
            *pp = n->next;
            free(n);
            if (orphan && !read_only) orphan_remove(ino);
            maybe_free_inode(ino);
        }
        return;
    }
}

// After a crash: the orphan list's inodes are freed (or, with names still,
// truncated to their size)
static void process_orphans(void) {
    uint32_t ino = le32(sb + 232), done = 0;
    while (ino && done < 100000) {
        struct cblk *c;
        uint8_t *p = ino_ptr(ino, &c);
        if (!p || ino < first_ino) break;
        uint32_t next = le32(p + 20);
        put32(p + 20, 0);
        c->dirty = 1;
        if (!le16(p + 26)) {
            if (le16(p)) release_inode(ino);
        } else if ((le16(p) & S_IFMT_) == S_IFREG_) {
            free_from(ino, (i_size(ino) + bs - 1) / bs);
        }
        done++;
        ino = next;
    }
    put32(sb + 232, 0);
    sb_dirty = 1;
    if (done) fprintf(stderr, "ext2fs: %s: %u orphan inode%s cleaned up\n", dev_name, done, done == 1 ? "" : "s");
}

// A new inode: mode, owner root, times now, one link (two for directories);
// files and directories use extents where the filesystem has them
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
    if ((feat_incompat & INCOMPAT_EXTENTS) && (mode & S_IFMT_) != S_IFLNK_) x_init_root(p);
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
    if (is_fast_link(p)) {
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
    if (le32(p + 32) & HUGE_FILE_FL) a->blocks *= bs / 512;
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
    uint64_t max = max_size(o->ino);
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
    if (!dir_link_ok(parent)) return -EMLINK;
    uint32_t ino = make_inode(parent, S_IFDIR_ | (mode & 07777));
    if (!ino) return -ENOSPC;
    int fresh;
    int64_t pb = bmap(ino, 0, 1, 0, &fresh);
    if (pb <= 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return -ENOSPC;
    }
    struct cblk *c = dir_new_block(ino, (uint32_t)pb);
    if (!c) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return -EIO;
    }
    put_entry(c->data, 12, ".", 1, ino, 2);
    put_entry(c->data + 12, (csum ? bs - 12 : bs) - 12, "..", 2, parent, 2);
    set_size_field(ino, bs);
    r = dir_add(parent, last, ll, ino, 2);
    if (r < 0) {
        set_links(ino, 0);
        maybe_free_inode(ino);
        return r;
    }
    dir_link_inc(parent);
    return 0;
}

// Take a name away from an inode (`isdir`: a directory's own "." and its
// parent's ".." link go too)
static void drop_name(uint32_t parent, uint32_t ino, int isdir) {
    if (isdir) {
        set_links(ino, 0);
        dir_link_dec(parent);
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
        if (isdir && pa != pb && !dir_link_ok(pb)) return -EMLINK;
        r = dir_add(pb, nb, lb, ds.ino, type);
        if (r < 0) return r;
    }
    // The old entry (found again: adding may have moved it within its block)
    if (dir_find(pa, na, la, &ds) == 0) dir_remove(pa, ds.off);
    if (isdir && pa != pb) {
        // Its ".." now names the new parent
        struct dent dot;
        if (dir_find(ds.ino, "..", 2, &dot) == 0) dir_retarget(ds.ino, dot.off, pb, 2);
        dir_link_inc(pb);
        dir_link_dec(pa);
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
    } else {
        if (feat_incompat & INCOMPAT_EXTENTS) {
            struct cblk *c;
            uint8_t *p = ino_ptr(ino, &c);
            x_init_root(p);
            c->dirty = 1;
        }
        r = file_io(ino, 0, (uint8_t *)target, tl, 1);
    }
    if (r < 0) {
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
    strcpy(s->type, (feat_incompat & (INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_FLEX_BG)) ? "ext4" : (feat_compat & COMPAT_HAS_JOURNAL) ? "ext3" : "ext2");
}

static int shutting_down;

// Cleanly unmounted: everything written, the journal empty, the
// superblock says so
static int unmount_clean(void) {
    int r = flush();
    if (read_only) return r;
    if (journaling) {
        putbe32(jb.sbb + 0x18, j_seq);
        putbe32(jb.sbb + 0x1C, 0);
        jbd_sb_seal(&jb);
        memcpy(iobuf, jb.sbb, bs);
        if (dev_rw(1, blk_dev(jb.map[0]), spb) < 0) r = -EIO;
        dev_flush();
        feat_incompat &= ~(uint32_t)INCOMPAT_RECOVER;
        put32(sb + 96, feat_incompat);
    }
    put16(sb + 58, le16(sb + 58) | 1);
    put32(sb + 48, (uint32_t)(now_ns() / 1000000000));
    if (write_super() < 0) r = -EIO;
    dev_flush();
    return r;
}

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
        if (unmount_clean() < 0) {
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

static int jrd(void *ctx, uint32_t b, uint8_t *buf) {
    (void)ctx;
    if (b >= nblocks || dev_rw(0, blk_dev(b), spb) < 0) return -EIO;
    memcpy(buf, iobuf, bs);
    return 0;
}

static int jwr(void *ctx, uint32_t b, const uint8_t *buf) {
    (void)ctx;
    if (b >= nblocks) return -EIO;
    memcpy(iobuf, buf, bs);
    return dev_rw(1, blk_dev(b), spb);
}

// The journal inode's blocks
static int journal_map(void) {
    j_ino = le32(sb + 224);
    if (!j_ino || j_ino > ninodes) return -EINVAL;
    uint64_t len = i_size(j_ino) / bs;
    if (len < 1024 || len > (1u << 24)) return -EINVAL;
    free(jb.map);
    jb.map = malloc(len * 4);
    if (!jb.map) return -ENOMEM;
    for (uint32_t k = 0; k < len; k++) {
        int64_t pb = bmap(j_ino, k, 0, 0, 0);
        if (pb <= 0) return -EINVAL;
        jb.map[k] = (uint32_t)pb;
    }
    jb.len = (uint32_t)len;
    jb.bs = bs;
    jb.rd = jrd;
    jb.wr = jwr;
    free(jb.buf);
    free(jb.sbb);
    return jbd_open(&jb);
}

static int read_super(void) {
    uint64_t first = 1024 / dev_bs, count = (2048 + dev_bs - 1) / dev_bs - first;
    if (dev_rw(0, first, (uint32_t)count) < 0) return -EIO;
    memcpy(sb, iobuf + 1024 % dev_bs, 1024);
    return 0;
}

// What the superblock says; -EINVAL if it is not one we can use
static int parse_super(void) {
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
    if (le32(sb + 76) >= 1) {
        isz = le16(sb + 88);
        first_ino = le32(sb + 84);
        feat_compat = le32(sb + 92);
        feat_incompat = le32(sb + 96);
        feat_ro = le32(sb + 100);
    } else {
        isz = 128;
        first_ino = 11;
        feat_compat = feat_incompat = feat_ro = 0;
    }
    if (feat_incompat & INCOMPAT_64BIT) {
        if (le32(sb + 336)) {
            fprintf(stderr, "ext2fs: %s: more than 2^32 blocks: not served here\n", dev_name);
            return -ENOTSUP;
        }
        desc_size = le16(sb + 254);
        if (desc_size < 32 || desc_size > 1024 || (desc_size & (desc_size - 1))) return -EINVAL;
    } else {
        desc_size = 32;
    }
    if (!bpg || !ipg || bpg > bs * 8 || ipg > bs * 8 || nblocks <= first_data) return -EINVAL;
    if ((uint64_t)nblocks * spb > dev_blocks) return -EINVAL;
    ngroups = (nblocks - first_data + bpg - 1) / bpg;
    if ((uint64_t)ngroups * ipg < ninodes) return -EINVAL;
    if (isz < 128 || isz > bs || (isz & (isz - 1))) return -EINVAL;
    gdt_block = first_data + 1;
    sb_home = 1024 / bs;
    sb_off = 1024 % bs;
    x4_setup(&X, sb);
    csum = X.csum;
    uninit_ok = X.csum || X.gdt_csum;
    uint32_t known = INCOMPAT_FILETYPE | INCOMPAT_RECOVER | INCOMPAT_EXTENTS | INCOMPAT_64BIT | INCOMPAT_FLEX_BG |
                     INCOMPAT_CSUM_SEED | INCOMPAT_LARGEDIR;
    if (feat_incompat & ~known) {
        fprintf(stderr, "ext2fs: %s: has features this server cannot read (incompat %#x)\n", dev_name, feat_incompat & ~known);
        return -ENOTSUP;
    }
    if (csum && le32(sb + 1020) != x4_sb_csum(sb)) {
        fprintf(stderr, "ext2fs: %s: the superblock's checksum is wrong (run e2fsck)\n", dev_name);
        return -EINVAL;
    }
    return 0;
}

static int load(void) {
    int replayed = 0;
again:
    if (read_super() < 0) return -EIO;
    int r = parse_super();
    if (r < 0) return r;
    if (!cache[0].data) {
        for (int i = 0; i < CACHE; i++) {
            cache[i].data = malloc(4096);
            if (!cache[i].data) return -ENOMEM;
        }
        sbblk = malloc(4096);
        jtmp = malloc(4096);
        if (!sbblk || !jtmp) return -ENOMEM;
    }
    drop_cache();
    uint8_t *d = gd(0, 0);
    if (!d || le32(d + 8) >= nblocks) return -EINVAL;
    if (!is_dir_ino(2)) return -EINVAL;
    // Features this server does not keep up to date: read-only
    uint32_t ro_known = RO_SPARSE_SUPER | RO_LARGE_FILE | RO_HUGE_FILE | RO_GDT_CSUM | RO_DIR_NLINK | RO_EXTRA_ISIZE |
                        RO_METADATA_CSUM;
    const char *why = 0;
    if (feat_ro & ~ro_known & ~(uint32_t)RO_ORPHAN_PRESENT) why = "features this server does not keep up to date";
    else if (feat_ro & RO_ORPHAN_PRESENT) why = "inodes in its orphan file (e2fsck cleans them up)";
    else if (feat_compat & COMPAT_SPARSE_SUPER2) why = "sparse_super2";
    // The journal
    int has_journal = (feat_compat & COMPAT_HAS_JOURNAL) != 0;
    if (has_journal) {
        r = journal_map();
        if (r < 0) {
            fprintf(stderr, "ext2fs: %s: its journal cannot be used (%s)\n", dev_name, strerror(-r));
            if (feat_incompat & INCOMPAT_RECOVER) return -ENOTSUP;
            if (!why) why = "a journal this server cannot use";
            has_journal = 0;
        } else if (jbd_unknown(&jb) || (jb.incompat & JBD_INCOMPAT_FAST)) {
            if (feat_incompat & INCOMPAT_RECOVER) {
                fprintf(stderr, "ext2fs: %s: its journal has features this server cannot replay (%#x)\n", dev_name, jb.incompat);
                return -ENOTSUP;
            }
            if (!why) why = "journal features this server does not know";
            has_journal = 0;
        }
    }
    if (feat_incompat & INCOMPAT_RECOVER) {
        if (!has_journal) return -ENOTSUP;
        if (dev_read_only) {
            fprintf(stderr, "ext2fs: %s: needs its journal replayed, but the disk is read-only\n", dev_name);
            return -EROFS;
        }
        if (jb.start && !replayed) {
            r = jbd_replay(&jb);
            if (r < 0) {
                fprintf(stderr, "ext2fs: %s: replaying the journal failed (%s)\n", dev_name, strerror(-r));
                return r;
            }
            dev_flush();
            fprintf(stderr, "ext2fs: %s: journal replayed: %u transaction%s, %u blocks%s\n", dev_name, jb.transactions,
                    jb.transactions == 1 ? "" : "s", jb.replayed, jb.bad_blocks ? " (some damaged: run e2fsck)" : "");
            replayed = 1;
            goto again;   // the superblock itself may have come from the journal
        }
        // Brought up to date: no longer needs recovery
        feat_incompat &= ~(uint32_t)INCOMPAT_RECOVER;
        put32(sb + 96, feat_incompat);
        if (write_super() < 0) return -EIO;
        dev_flush();
    }
    if (why && !read_only) {
        fprintf(stderr, "ext2fs: %s: has %s: read-only\n", dev_name, why);
        read_only = 1;
    }
    // A filesystem id from the UUID (not 1-3: fsd's own)
    fsid = le32(sb + 104) ^ le32(sb + 108) ^ le32(sb + 112) ^ le32(sb + 116);
    if (fsid < 16) fsid += 16;
    if (read_only) return 0;
    uint32_t state = le16(sb + 58);
    if (has_journal) {
        // Journal checksums as Linux sets them for this filesystem
        uint32_t inc = jb.incompat;
        if (csum) inc = (inc & ~(uint32_t)JBD_INCOMPAT_CSUM_V2) | JBD_INCOMPAT_CSUM_V3;
        if (feat_incompat & INCOMPAT_64BIT) inc |= JBD_INCOMPAT_64BIT;
        if (inc != jb.incompat && be32(jb.sbb + 4) == JBD_SB_V2) {
            putbe32(jb.sbb + 0x28, inc);
            if (inc & JBD_INCOMPAT_CSUM_V3) {
                putbe32(jb.sbb + 0x24, be32(jb.sbb + 0x24) & ~(uint32_t)JBD_COMPAT_CHECKSUM);
                jb.sbb[0x50] = 4;   // crc32c
            }
            jbd_features(&jb);
        }
        jb.start = 0;
        putbe32(jb.sbb + 0x1C, 0);
        jbd_sb_seal(&jb);
        if (jwr(0, jb.map[0], jb.sbb) < 0) return -EIO;
        j_seq = jb.seq;
        uint32_t cap = j_capacity();
        if (cap < 64) return -EINVAL;
        max_dirty = cap - 2 < CACHE - 32 ? cap - 2 : CACHE - 32;
        journaling = 1;
        if (!(state & 1) || (state & 2))
            fprintf(stderr, "ext2fs: %s: %s (run fsck.ext4 or e2fsck)\n", dev_name, (state & 2) ? "has errors" : "was not checked after an error");
        // Needs recovery while mounted (until it is unmounted cleanly)
        feat_incompat |= INCOMPAT_RECOVER;
        put32(sb + 96, feat_incompat);
    } else {
        if (!(state & 1) || (state & 2))
            fprintf(stderr, "ext2fs: %s: was not cleanly unmounted%s (run fsck.ext2 or e2fsck)\n", dev_name,
                    (state & 2) ? " and has errors" : "");
        // Not clean while mounted
        put16(sb + 58, state & ~1u);
    }
    // One more mount
    put16(sb + 52, le16(sb + 52) + 1u);
    put32(sb + 44, (uint32_t)(now_ns() / 1000000000));
    if (write_super() < 0) return -EIO;
    // The superblock's block, for the journal
    if (journaling) {
        if (dev_rw(0, blk_dev(sb_home), spb) < 0) return -EIO;
        memcpy(sbblk, iobuf, bs);
    }
    if (le32(sb + 232)) {
        process_orphans();
        if (flush() < 0) return -EIO;
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
    if (info.flags & RX_BLOCK_FLAG_READONLY) read_only = dev_read_only = 1;
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
        if (r == -EINVAL || r == -EIO) fprintf(stderr, "ext2fs: %s: not an ext2/3/4 filesystem%s\n", dev_name, r == -EIO ? " (read error)" : "");
        return 1;
    }
    struct ofd *o = new_ofd(2, O_RDONLY | O_DIRECTORY, "", 0);
    add_conn(first, o);

    static rx_wait_item_t items[MAX_CONNS];
    for (;;) {
        int n = nconns;
        if (n == 0 || shutting_down) {
            if (!shutting_down) unmount_clean();
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
