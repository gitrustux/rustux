// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ext4lib.h: what ext2fs, fsck.ext2, mkfs.ext4 and tune2fs share about
// ext2/ext3/ext4 (step 16): byte order, feature bits, the checksums
// (crc32c for metadata_csum, crc16 for the older uninit_bg) and the
// journal (jbd2): its format and replaying it. Included by each program
// (the build compiles one program per .c file).
//
// Formats: Linux's Documentation/filesystems/ext4/ and
// include/linux/jbd2.h; checksums as fs/ext4 and fs/jbd2 compute them.

#ifndef EXT4LIB_H
#define EXT4LIB_H

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#define X4F static __attribute__((unused))

X4F inline uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }
X4F inline uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
X4F inline void put16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
X4F inline void put32(uint8_t *p, uint32_t v) { put16(p, v); put16(p + 2, v >> 16); }
X4F inline uint32_t be32(const uint8_t *p) { return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3]; }
X4F inline uint16_t be16(const uint8_t *p) { return (uint16_t)(p[0] << 8 | p[1]); }
X4F inline void putbe32(uint8_t *p, uint32_t v) { p[0] = (uint8_t)(v >> 24); p[1] = (uint8_t)(v >> 16); p[2] = (uint8_t)(v >> 8); p[3] = (uint8_t)v; }
X4F inline void putbe16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)(v >> 8); p[1] = (uint8_t)v; }

// ---- features (superblock) ----

#define COMPAT_DIR_PREALLOC   0x0001
#define COMPAT_HAS_JOURNAL    0x0004
#define COMPAT_EXT_ATTR       0x0008
#define COMPAT_RESIZE_INODE   0x0010
#define COMPAT_DIR_INDEX      0x0020
#define COMPAT_SPARSE_SUPER2  0x0200
#define COMPAT_FAST_COMMIT    0x0400
#define COMPAT_STABLE_INODES  0x0800
#define COMPAT_ORPHAN_FILE    0x1000

#define INCOMPAT_FILETYPE     0x0002
#define INCOMPAT_RECOVER      0x0004
#define INCOMPAT_JOURNAL_DEV  0x0008
#define INCOMPAT_META_BG      0x0010
#define INCOMPAT_EXTENTS      0x0040
#define INCOMPAT_64BIT        0x0080
#define INCOMPAT_MMP          0x0100
#define INCOMPAT_FLEX_BG      0x0200
#define INCOMPAT_EA_INODE     0x0400
#define INCOMPAT_CSUM_SEED    0x2000
#define INCOMPAT_LARGEDIR     0x4000
#define INCOMPAT_INLINE_DATA  0x8000

#define RO_SPARSE_SUPER       0x0001
#define RO_LARGE_FILE         0x0002
#define RO_HUGE_FILE          0x0008
#define RO_GDT_CSUM           0x0010
#define RO_DIR_NLINK          0x0020
#define RO_EXTRA_ISIZE        0x0040
#define RO_METADATA_CSUM      0x0400
#define RO_ORPHAN_PRESENT     0x10000

// Group descriptor flags
#define BG_INODE_UNINIT 0x0001
#define BG_BLOCK_UNINIT 0x0002
#define BG_INODE_ZEROED 0x0004

// Inode flags
#define INDEX_FL      0x1000      // hashed directory
#define HUGE_FILE_FL  0x40000     // i_blocks in filesystem blocks
#define EXTENTS_FL    0x80000
#define INLINE_FL     0x10000000

#define XMAGIC 0xF30A             // extent node header

// ---- checksums ----

static uint32_t crc32c_tab[256];
static uint16_t crc16_tab[256];

X4F void crc_init(void) {
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i, d = i;
        for (int k = 0; k < 8; k++) {
            c = c & 1 ? (c >> 1) ^ 0x82F63B78u : c >> 1;   // Castagnoli, reflected
            d = d & 1 ? (d >> 1) ^ 0xA001u : d >> 1;       // crc16 (ANSI), reflected
        }
        crc32c_tab[i] = c;
        crc16_tab[i] = (uint16_t)d;
    }
}

// crc32c without the final inversion: what ext4 and jbd2 call ext4_chksum
X4F uint32_t crc32c(uint32_t crc, const void *buf, size_t n) {
    const uint8_t *b = buf;
    if (!crc32c_tab[1]) crc_init();
    while (n--) crc = crc32c_tab[(crc ^ *b++) & 0xFF] ^ (crc >> 8);
    return crc;
}

X4F uint16_t crc16(uint16_t crc, const void *buf, size_t n) {
    const uint8_t *b = buf;
    if (!crc32c_tab[1]) crc_init();
    while (n--) crc = (uint16_t)((crc >> 8) ^ crc16_tab[(crc ^ *b++) & 0xFF]);
    return crc;
}

// What the checksums need to know about a filesystem
struct x4 {
    const uint8_t *sb;
    uint32_t bs, desc_size, isz, ipg, cpg;   // cpg: blocks (clusters) per group
    int csum;        // metadata_csum
    int gdt_csum;    // uninit_bg: crc16 group descriptor checksums
    uint32_t seed;   // crc32c seed: from the UUID, or the superblock's
};

X4F void x4_setup(struct x4 *x, const uint8_t *sb) {
    x->sb = sb;
    x->bs = 1024u << le32(sb + 24);
    uint32_t inc = le32(sb + 96), ro = le32(sb + 100);
    x->desc_size = (inc & INCOMPAT_64BIT) ? le16(sb + 254) : 32;
    if (x->desc_size < 32) x->desc_size = 32;
    x->isz = le32(sb + 76) ? le16(sb + 88) : 128;
    x->ipg = le32(sb + 40);
    x->cpg = le32(sb + 36) ? le32(sb + 36) : le32(sb + 32);
    x->csum = (ro & RO_METADATA_CSUM) != 0;
    x->gdt_csum = !x->csum && (ro & RO_GDT_CSUM);
    x->seed = (inc & INCOMPAT_CSUM_SEED) ? le32(sb + 624) : crc32c(~0u, sb + 104, 16);
}

X4F uint32_t x4_sb_csum(const uint8_t *sb) { return crc32c(~0u, sb, 1020); }

X4F uint16_t x4_gd_csum(const struct x4 *x, uint32_t g, const uint8_t *d) {
    uint8_t lg[4], zero[2] = { 0, 0 };
    put32(lg, g);
    if (x->csum) {
        uint32_t c = crc32c(x->seed, lg, 4);
        c = crc32c(c, d, 0x1E);
        c = crc32c(c, zero, 2);
        if (x->desc_size > 0x20) c = crc32c(c, d + 0x20, x->desc_size - 0x20);
        return (uint16_t)c;
    }
    if (!x->gdt_csum) return 0;
    uint16_t c = crc16(0xFFFF, x->sb + 104, 16);
    c = crc16(c, lg, 4);
    c = crc16(c, d, 0x1E);
    if (x->desc_size > 0x20) c = crc16(c, d + 0x20, x->desc_size - 0x20);
    return c;
}

// The seed of an inode's own checksums (its inode, extent and directory blocks)
X4F uint32_t x4_inode_seed(const struct x4 *x, uint32_t ino, uint32_t gen) {
    uint8_t b[4];
    put32(b, ino);
    uint32_t c = crc32c(x->seed, b, 4);
    put32(b, gen);
    return crc32c(c, b, 4);
}

X4F int x4_has_csum_hi(const struct x4 *x, const uint8_t *p) { return x->isz > 128 && 128u + le16(p + 128) >= 0x84; }

X4F uint32_t x4_inode_csum(const struct x4 *x, uint32_t ino, const uint8_t *p) {
    static const uint8_t zero[2];
    uint32_t c = x4_inode_seed(x, ino, le32(p + 100));
    c = crc32c(c, p, 0x7C);
    c = crc32c(c, zero, 2);
    c = crc32c(c, p + 0x7E, 128 - 0x7E);
    if (x->isz > 128) {
        c = crc32c(c, p + 128, 2);
        uint32_t off = 0x82;
        if (x4_has_csum_hi(x, p)) {
            c = crc32c(c, zero, 2);
            off = 0x84;
        }
        c = crc32c(c, p + off, x->isz - off);
    }
    return c;
}

X4F void x4_inode_csum_set(const struct x4 *x, uint32_t ino, uint8_t *p) {
    uint32_t c = x4_inode_csum(x, ino, p);
    put16(p + 0x7C, c);
    if (x4_has_csum_hi(x, p)) put16(p + 0x82, c >> 16);
}

X4F int x4_inode_csum_ok(const struct x4 *x, uint32_t ino, const uint8_t *p) {
    uint32_t c = x4_inode_csum(x, ino, p);
    if (le16(p + 0x7C) != (c & 0xFFFF)) return 0;
    return !x4_has_csum_hi(x, p) || le16(p + 0x82) == c >> 16;
}

// Extent blocks: the checksum after the last possible entry
X4F uint32_t x4_ext_tail(const uint8_t *h) { return 12 + 12u * le16(h + 4); }
X4F uint32_t x4_ext_csum(uint32_t iseed, const uint8_t *h) { return crc32c(iseed, h, x4_ext_tail(h)); }

// Directory blocks: a 12-byte entry at the end holds the checksum
X4F int x4_dir_has_tail(uint32_t bs, const uint8_t *b) {
    const uint8_t *t = b + bs - 12;
    return le32(t) == 0 && le16(t + 4) == 12 && t[6] == 0 && t[7] == 0xDE;
}
X4F void x4_dir_init_tail(uint32_t bs, uint8_t *b) {
    uint8_t *t = b + bs - 12;
    memset(t, 0, 12);
    put16(t + 4, 12);
    t[7] = 0xDE;
}
X4F uint32_t x4_dir_csum(uint32_t iseed, uint32_t bs, const uint8_t *b) { return crc32c(iseed, b, bs - 12); }

// Hashed-directory nodes: a tail after the limit of dx entries
// (count_offset: 0x20 in the root, 8 in other nodes)
X4F int x4_dx_tail(uint32_t bs, const uint8_t *b, uint32_t count_offset, uint32_t *tail_at, uint32_t *count) {
    uint32_t limit = le16(b + count_offset), n = le16(b + count_offset + 2);
    if (count_offset + limit * 8 + 8 > bs || n > limit) return 0;
    *tail_at = count_offset + limit * 8;
    *count = n;
    return 1;
}
X4F uint32_t x4_dx_csum(uint32_t iseed, const uint8_t *b, uint32_t count_offset, uint32_t count, uint32_t tail_at) {
    uint32_t c = crc32c(iseed, b, count_offset + count * 8);
    return crc32c(c, b + tail_at, 4);
}

X4F uint32_t x4_xattr_csum(const struct x4 *x, uint32_t block, const uint8_t *b) {
    uint8_t nr[8] = { 0 }, zero[4] = { 0 };
    put32(nr, block);
    uint32_t c = crc32c(x->seed, nr, 8);
    c = crc32c(c, b, 0x10);
    c = crc32c(c, zero, 4);
    return crc32c(c, b + 0x14, x->bs - 0x14);
}

X4F uint32_t x4_bitmap_csum(const struct x4 *x, const uint8_t *bm, uint32_t bits) { return crc32c(x->seed, bm, bits / 8); }

// Groups with a superblock copy (sparse_super): 0, 1 and powers of 3, 5, 7
X4F int x4_has_super(const uint8_t *sb, uint32_t g) {
    if (!(le32(sb + 100) & RO_SPARSE_SUPER)) return 1;
    if (g <= 1) return 1;
    for (uint32_t b = 3; b <= 7; b += 2) {
        uint64_t p = b;
        while (p < g) p *= b;
        if (p == g) return 1;
    }
    return 0;
}

// ---- the journal (jbd2) ----

#define JBD_MAGIC      0xC03B3998u
#define JBD_DESCRIPTOR 1
#define JBD_COMMIT     2
#define JBD_SB_V1      3
#define JBD_SB_V2      4
#define JBD_REVOKE     5

#define JBD_COMPAT_CHECKSUM   0x1
#define JBD_INCOMPAT_REVOKE   0x1
#define JBD_INCOMPAT_64BIT    0x2
#define JBD_INCOMPAT_ASYNC    0x4
#define JBD_INCOMPAT_CSUM_V2  0x8
#define JBD_INCOMPAT_CSUM_V3  0x10
#define JBD_INCOMPAT_FAST     0x20

#define JBD_FLAG_ESCAPE    1
#define JBD_FLAG_SAME_UUID 2
#define JBD_FLAG_LAST_TAG  8

struct jbd {
    uint32_t bs;
    uint32_t *map;          // journal block -> filesystem block
    uint32_t len;           // blocks in the journal inode
    uint8_t *sbb;           // journal block 0 (the journal superblock in its first 1024 bytes)
    uint32_t maxlen, first, start, seq;
    uint32_t incompat, compat;
    int csum;               // 2 or 3: checksum version, 0: none
    uint32_t seed;
    uint32_t tag_bytes;
    uint8_t *buf;           // one block
    int (*rd)(void *ctx, uint32_t blk, uint8_t *buf);
    int (*wr)(void *ctx, uint32_t blk, const uint8_t *buf);
    void *ctx;
    // replay results
    uint32_t replayed, revoked, bad_blocks, transactions;
};

X4F void jbd_features(struct jbd *j) {
    uint8_t *s = j->sbb;
    j->compat = be32(s + 4) == JBD_SB_V2 ? be32(s + 0x24) : 0;
    j->incompat = be32(s + 4) == JBD_SB_V2 ? be32(s + 0x28) : 0;
    j->csum = (j->incompat & JBD_INCOMPAT_CSUM_V3) ? 3 : (j->incompat & JBD_INCOMPAT_CSUM_V2) ? 2 : 0;
    if (j->csum == 3) j->tag_bytes = 16;
    else j->tag_bytes = 12 + (j->csum == 2 ? 2 : 0) - ((j->incompat & JBD_INCOMPAT_64BIT) ? 0 : 4);
    j->seed = crc32c(~0u, s + 0x30, 16);
}

X4F uint32_t jbd_sb_csum(const uint8_t *s) {
    uint8_t tmp[1024];
    memcpy(tmp, s, 1024);
    memset(tmp + 0xFC, 0, 4);
    return crc32c(~0u, tmp, 1024);
}

// Read and check the journal superblock: 0, or -EINVAL (not a journal
// this code knows), -EIO
X4F int jbd_open(struct jbd *j) {
    if (!j->len) return -EINVAL;
    j->buf = malloc(j->bs);
    j->sbb = malloc(j->bs);
    if (!j->buf || !j->sbb) return -ENOMEM;
    if (j->rd(j->ctx, j->map[0], j->sbb) < 0) return -EIO;
    uint8_t *s = j->sbb;
    uint32_t t = be32(s + 4);
    if (be32(s) != JBD_MAGIC || (t != JBD_SB_V1 && t != JBD_SB_V2)) return -EINVAL;
    if (be32(s + 0xC) != j->bs) return -EINVAL;
    j->maxlen = be32(s + 0x10);
    j->first = be32(s + 0x14);
    j->seq = be32(s + 0x18);
    j->start = be32(s + 0x1C);
    if (j->maxlen > j->len || j->first == 0 || j->first >= j->maxlen) return -EINVAL;
    jbd_features(j);
    if (j->csum && s[0x50] != 4) return -EINVAL;   // checksum type: crc32c
    if (j->csum && be32(s + 0xFC) != jbd_sb_csum(s)) return -EINVAL;
    return 0;
}

X4F int jbd_unknown(const struct jbd *j) {
    return j->incompat & ~(uint32_t)(JBD_INCOMPAT_REVOKE | JBD_INCOMPAT_64BIT | JBD_INCOMPAT_ASYNC | JBD_INCOMPAT_CSUM_V2 | JBD_INCOMPAT_CSUM_V3);
}

X4F void jbd_sb_seal(struct jbd *j) {
    if (j->csum) putbe32(j->sbb + 0xFC, jbd_sb_csum(j->sbb));
}

X4F uint32_t jbd_next(const struct jbd *j, uint32_t b) { return b + 1 >= j->maxlen ? j->first : b + 1; }

X4F int jbd_block_csum_ok(const struct jbd *j, uint8_t *b, uint32_t at) {
    uint32_t got = be32(b + at);
    memset(b + at, 0, 4);
    uint32_t want = crc32c(j->seed, b, j->bs);
    putbe32(b + at, got);
    return got == want;
}

// The tags of a descriptor block: up to `max`, with their flags and checksums
X4F int jbd_tags(const struct jbd *j, const uint8_t *b, uint32_t *blk, uint32_t *flags, uint32_t *csum, int max) {
    uint32_t off = 12, end = j->bs - (j->csum ? 4 : 0);
    int n = 0;
    while (off + j->tag_bytes <= end && n < max) {
        const uint8_t *t = b + off;
        uint32_t fl, hi = 0;
        if (j->csum == 3) {
            fl = be32(t + 4);
            hi = be32(t + 8);
            csum[n] = be32(t + 12);
        } else {
            csum[n] = be16(t + 4);
            fl = be16(t + 6);
            if (j->incompat & JBD_INCOMPAT_64BIT) hi = be32(t + 8);
        }
        blk[n] = hi ? 0xFFFFFFFFu : be32(t);   // beyond 32 bits: never written here
        flags[n] = fl;
        n++;
        off += j->tag_bytes;
        if (!(fl & JBD_FLAG_SAME_UUID)) off += 16;
        if (fl & JBD_FLAG_LAST_TAG) break;
    }
    return n;
}

// Revoked blocks: block -> the newest transaction that revoked it
struct jbd_rev { uint32_t blk, seq; struct jbd_rev *next; };
#define JBD_REVHASH 1024

X4F void jbd_rev_add(struct jbd_rev **h, uint32_t blk, uint32_t seq) {
    struct jbd_rev **pp = &h[blk % JBD_REVHASH];
    for (struct jbd_rev *r = *pp; r; r = r->next)
        if (r->blk == blk) { if ((int32_t)(seq - r->seq) > 0) r->seq = seq; return; }
    struct jbd_rev *r = malloc(sizeof *r);
    if (!r) return;
    r->blk = blk;
    r->seq = seq;
    r->next = *pp;
    *pp = r;
}

X4F int jbd_revoked(struct jbd_rev **h, uint32_t blk, uint32_t seq) {
    for (struct jbd_rev *r = h[blk % JBD_REVHASH]; r; r = r->next)
        if (r->blk == blk) return (int32_t)(r->seq - seq) >= 0;
    return 0;
}

// Replay the journal (as jbd2's recovery does: a scan for the last
// complete transaction, the revoke records, then the blocks), then mark
// it empty. 0, or a negative errno. A journal that is already empty does
// nothing.
X4F int jbd_replay(struct jbd *j) {
    j->replayed = j->revoked = j->bad_blocks = j->transactions = 0;
    if (!j->start) return 0;
    if (jbd_unknown(j) || (j->incompat & JBD_INCOMPAT_FAST)) return -ENOTSUP;
    int maxtags = (int)(j->bs / 8);
    uint32_t *tb = malloc(maxtags * 4 * 3);
    struct jbd_rev **rev = calloc(JBD_REVHASH, sizeof *rev);
    uint8_t *data = malloc(j->bs);
    if (!tb || !rev || !data) return -ENOMEM;
    uint32_t *tblk = tb, *tfl = tb + maxtags, *tcs = tb + 2 * maxtags;
    uint32_t end_seq = j->seq;
    int r = 0;
    for (int pass = 0; pass < 3 && r == 0; pass++) {
        uint32_t seq = j->seq, b = j->start;
        for (uint32_t guard = 0; guard < j->maxlen * 2; guard++) {
            if (pass > 0 && seq == end_seq) break;
            if (j->rd(j->ctx, j->map[b], j->buf) < 0) { r = -EIO; break; }
            if (be32(j->buf) != JBD_MAGIC || be32(j->buf + 8) != seq) break;
            uint32_t type = be32(j->buf + 4);
            if (type == JBD_DESCRIPTOR) {
                if (j->csum && !jbd_block_csum_ok(j, j->buf, j->bs - 4)) break;
                int n = jbd_tags(j, j->buf, tblk, tfl, tcs, maxtags);
                b = jbd_next(j, b);
                for (int k = 0; k < n; k++, b = jbd_next(j, b)) {
                    if (pass != 2) continue;
                    if (j->rd(j->ctx, j->map[b], data) < 0) { r = -EIO; break; }
                    if (j->csum) {
                        uint8_t sq[4];
                        putbe32(sq, seq);
                        uint32_t c = crc32c(crc32c(j->seed, sq, 4), data, j->bs);
                        if ((j->csum == 3 ? c : (c & 0xFFFF)) != tcs[k]) { j->bad_blocks++; continue; }
                    }
                    if (tblk[k] == 0xFFFFFFFFu) { j->bad_blocks++; continue; }
                    if (jbd_revoked(rev, tblk[k], seq)) { j->revoked++; continue; }
                    if (tfl[k] & JBD_FLAG_ESCAPE) putbe32(data, JBD_MAGIC);
                    if (j->wr(j->ctx, tblk[k], data) < 0) { r = -EIO; break; }
                    j->replayed++;
                }
                if (r) break;
            } else if (type == JBD_COMMIT) {
                if (pass == 0 && j->csum && !jbd_block_csum_ok(j, j->buf, 16)) break;
                seq++;
                if (pass == 0) { end_seq = seq; j->transactions++; }
                b = jbd_next(j, b);
            } else if (type == JBD_REVOKE) {
                if (j->csum && !jbd_block_csum_ok(j, j->buf, j->bs - 4)) break;
                if (pass == 1) {
                    uint32_t count = be32(j->buf + 12), w = (j->incompat & JBD_INCOMPAT_64BIT) ? 8 : 4;
                    if (count > j->bs) count = j->bs;
                    for (uint32_t off = 16; off + w <= count; off += w) {
                        uint32_t blk = w == 8 ? (be32(j->buf + off) ? 0xFFFFFFFFu : be32(j->buf + off + 4)) : be32(j->buf + off);
                        jbd_rev_add(rev, blk, seq);
                    }
                }
                b = jbd_next(j, b);
            } else {
                break;
            }
        }
    }
    for (int i = 0; i < JBD_REVHASH; i++)
        while (rev[i]) { struct jbd_rev *n = rev[i]->next; free(rev[i]); rev[i] = n; }
    free(rev);
    free(tb);
    free(data);
    if (r) return r;
    // Empty now; the next transaction gets a higher number
    j->seq = end_seq + 1;
    j->start = 0;
    putbe32(j->sbb + 0x18, j->seq);
    putbe32(j->sbb + 0x1C, 0);
    jbd_sb_seal(j);
    return j->wr(j->ctx, j->map[0], j->sbb);
}

#endif
