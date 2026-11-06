// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fatfs: the FAT filesystem server (step 14). Serves a FAT12, FAT16 or
// FAT32 filesystem on a block device with the file protocol of fsd
// (<rustux/fs.h>), read and write, long file names (VFAT) included.
//
//   fatfs [-r] DEVICE        started by mount, with the server end of the
//                            root directory's connection as PA_USER0
//
// The mount tool hands the other end to fsd (RX_FS_OP_MOUNT); fsd
// forwards path requests below the mount point, and opened files talk to
// fatfs directly. Changed metadata (FAT, directories, FSInfo) is written to
// the disk before each request is answered, so a pulled stick loses at
// most the request in flight. Times are stored in local time, as Windows
// expects. Specification: Microsoft "FAT32 File System Specification"
// (1.03), and the VFAT long-name format.

#include <errno.h>
#include <fcntl.h>
#include <rustux/device.h>
#include <rustux/fs.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define MAX_CONNS 500
#define IOBUF (256u * 1024u)
#define CACHE 512           // metadata sectors kept

#define ATTR_RO     0x01
#define ATTR_HIDDEN 0x02
#define ATTR_SYS    0x04
#define ATTR_VOLUME 0x08
#define ATTR_DIR    0x10
#define ATTR_ARCH   0x20
#define ATTR_LFN    0x0F

static const char *dev_name = "?";
static int read_only;

// ---- the block device ----

static rx_handle_t blk;
static uint32_t dev_bs;           // device block size
static uint64_t dev_blocks;
static uint8_t *iobuf;            // mapped VMO attached to the device
static uint32_t vmoid;

static int dev_rw(int write, uint64_t block, uint32_t count) {
    rx_block_req_t q = { 0 };
    q.op = write ? RX_BLOCK_OP_WRITE_VMO : RX_BLOCK_OP_READ_VMO;
    q.block = block;
    q.count = count;
    q.vmoid = vmoid;
    q.vmo_offset = 0;
    int r = rx_block_call(blk, &q, 0, 0, 0, 0, 0, 0);
    return r == -EPIPE ? -EIO : r;   // the disk is gone (unplugged)
}

// ---- the filesystem's layout ----

static int fat_type;              // 12, 16 or 32
static uint32_t ssz;              // bytes per sector
static uint32_t spb;              // device blocks per sector
static uint32_t spc;              // sectors per cluster
static uint32_t csz;              // bytes per cluster
static uint32_t fat_start, fat_sectors, nfats;
static uint32_t root_start, root_sectors;   // FAT12/16 fixed root directory
static uint32_t data_start;
static uint32_t nclusters;        // data clusters: 2 .. nclusters + 1
static uint32_t root_cluster;     // FAT32
static uint32_t fsinfo_sector;
static uint32_t free_count = 0xFFFFFFFF, next_free = 2;
static int fsinfo_dirty;
static uint32_t serial;
static char label[12];

static uint32_t eoc(void) { return fat_type == 32 ? 0x0FFFFFFF : fat_type == 16 ? 0xFFFF : 0xFFF; }
static int is_eoc(uint32_t v) { return v >= (fat_type == 32 ? 0x0FFFFFF8u : fat_type == 16 ? 0xFFF8u : 0xFF8u); }
static int valid_cluster(uint32_t c) { return c >= 2 && c < nclusters + 2; }
static uint64_t cluster_sector(uint32_t c) { return data_start + (uint64_t)(c - 2) * spc; }

// ---- sector cache (FAT and directories) ----

struct csec {
    uint64_t sector;    // UINT64_MAX: empty
    uint64_t used;      // for LRU
    int dirty;
    uint8_t *data;
};

static struct csec cache[CACHE];
static uint64_t cache_clock;
static int io_error;              // a write failed: reported by the request

static int sector_io(int write, uint64_t sector, uint32_t count, void *buf) {
    if ((uint64_t)count * ssz > IOBUF) return -EINVAL;
    if (write) memcpy(iobuf, buf, (size_t)count * ssz);
    int r = dev_rw(write, sector * spb, count * spb);
    if (r == 0 && !write) memcpy(buf, iobuf, (size_t)count * ssz);
    return r;
}

// Write a FAT sector to every copy of the FAT
static int write_cached(struct csec *c) {
    int r = sector_io(1, c->sector, 1, c->data);
    if (c->sector >= fat_start && c->sector < fat_start + fat_sectors) {
        for (uint32_t k = 1; k < nfats && r == 0; k++) r = sector_io(1, c->sector + (uint64_t)k * fat_sectors, 1, c->data);
    }
    if (r == 0) c->dirty = 0;
    else io_error = r;
    return r;
}

static struct csec *get_sector(uint64_t sector) {
    struct csec *victim = 0;
    for (int i = 0; i < CACHE; i++) {
        if (cache[i].sector == sector) {
            cache[i].used = ++cache_clock;
            return &cache[i];
        }
        if (!victim || cache[i].used < victim->used) victim = &cache[i];
    }
    if (victim->dirty) write_cached(victim);
    victim->sector = UINT64_MAX;
    if (sector_io(0, sector, 1, victim->data) < 0) {
        io_error = -EIO;
        return 0;
    }
    victim->sector = sector;
    victim->used = ++cache_clock;
    victim->dirty = 0;
    return victim;
}

static void put_le16(uint8_t *p, uint32_t v) { p[0] = (uint8_t)v; p[1] = (uint8_t)(v >> 8); }
static void put_le32(uint8_t *p, uint32_t v) { put_le16(p, v); put_le16(p + 2, v >> 16); }
static uint32_t le16(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8; }
static uint32_t le32(const uint8_t *p) { return le16(p) | le16(p + 2) << 16; }

static void write_fsinfo(void) {
    if (fat_type != 32 || !fsinfo_sector || !fsinfo_dirty) return;
    struct csec *c = get_sector(fsinfo_sector);
    if (!c) return;
    if (le32(c->data) == 0x41615252 && le32(c->data + 484) == 0x61417272) {
        put_le32(c->data + 488, free_count);
        put_le32(c->data + 492, next_free);
        c->dirty = 1;
    }
    fsinfo_dirty = 0;
}

// Everything changed goes to the disk, in sector order
static int write_pending;         // file data written since the last flush

static int flush(void) {
    write_pending = 0;
    write_fsinfo();
    for (;;) {
        struct csec *lo = 0;
        for (int i = 0; i < CACHE; i++) {
            if (cache[i].dirty && (!lo || cache[i].sector < lo->sector)) lo = &cache[i];
        }
        if (!lo) break;
        if (write_cached(lo) < 0) break;
    }
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_FLUSH;
    rx_block_call(blk, &q, 0, 0, 0, 0, 0, 0);
    int r = io_error;
    io_error = 0;
    return r;
}

// ---- the FAT ----

static int fat_byte(uint32_t off, uint8_t **p, struct csec **cs) {
    struct csec *c = get_sector(fat_start + off / ssz);
    if (!c) return -EIO;
    *p = c->data + off % ssz;
    *cs = c;
    return 0;
}

static uint32_t fat_get(uint32_t c) {
    uint8_t *p, *q;
    struct csec *s, *t;
    if (fat_type == 32) {
        if (fat_byte(c * 4, &p, &s)) return 0x0FFFFFF7;
        return le32(p) & 0x0FFFFFFF;
    }
    if (fat_type == 16) {
        if (fat_byte(c * 2, &p, &s)) return 0xFFF7;
        return le16(p);
    }
    uint32_t off = c + c / 2;   // FAT12: 12 bits, maybe across two sectors
    if (fat_byte(off, &p, &s)) return 0xFF7;
    uint32_t lo = *p;
    if (fat_byte(off + 1, &q, &t)) return 0xFF7;
    uint32_t v = lo | (uint32_t)*q << 8;
    return c & 1 ? v >> 4 : v & 0xFFF;
}

static void fat_set(uint32_t c, uint32_t v) {
    uint8_t *p, *q;
    struct csec *s, *t;
    uint32_t old = fat_get(c);
    if (fat_type == 32) {
        if (fat_byte(c * 4, &p, &s)) return;
        put_le32(p, (le32(p) & 0xF0000000) | (v & 0x0FFFFFFF));
        s->dirty = 1;
    } else if (fat_type == 16) {
        if (fat_byte(c * 2, &p, &s)) return;
        put_le16(p, v);
        s->dirty = 1;
    } else {
        uint32_t off = c + c / 2;
        if (fat_byte(off, &p, &s)) return;
        if (c & 1) *p = (uint8_t)((*p & 0x0F) | (v << 4 & 0xF0));
        else *p = (uint8_t)v;
        s->dirty = 1;
        if (fat_byte(off + 1, &q, &t)) return;
        if (c & 1) *q = (uint8_t)(v >> 4);
        else *q = (uint8_t)((*q & 0xF0) | (v >> 8 & 0x0F));
        t->dirty = 1;
    }
    if (free_count != 0xFFFFFFFF) {
        if (old == 0 && v != 0) free_count--;
        else if (old != 0 && v == 0) free_count++;
        fsinfo_dirty = 1;
    }
}

static void count_free(void) {
    uint32_t n = 0;
    for (uint32_t c = 2; c < nclusters + 2; c++) if (fat_get(c) == 0) n++;
    free_count = n;
}

// A free cluster marked end-of-chain (linked after `prev` if not 0); 0 if
// the disk is full
static uint32_t alloc_cluster(uint32_t prev) {
    if (free_count == 0) return 0;
    uint32_t start = valid_cluster(next_free) ? next_free : 2;
    uint32_t c = start;
    do {
        if (fat_get(c) == 0) {
            fat_set(c, eoc());
            if (prev) fat_set(prev, c);
            next_free = c + 1 < nclusters + 2 ? c + 1 : 2;
            fsinfo_dirty = 1;
            return c;
        }
        c = c + 1 < nclusters + 2 ? c + 1 : 2;
    } while (c != start);
    free_count = 0;
    return 0;
}

// Where dir_entry last was (a directory's cluster k is c); reset when
// clusters are freed
static uint32_t memo_dir, memo_k, memo_c;

static void free_chain(uint32_t c) {
    memo_dir = memo_k = memo_c = 0;
    for (int guard = 0; valid_cluster(c) && guard < (int)nclusters + 1; guard++) {
        uint32_t next = fat_get(c);
        fat_set(c, 0);
        if (c < next_free) next_free = c;
        if (is_eoc(next) || !valid_cluster(next)) break;
        c = next;
    }
}

static int zero_cluster(uint32_t c) {
    memset(iobuf, 0, csz);
    return dev_rw(1, cluster_sector(c) * spb, spc * spb);
}

// ---- times (stored in local time) ----

static void fat_time(int64_t ns, uint8_t *date_time_at /* 4 bytes: time, date */) {
    time_t t = (time_t)(ns / 1000000000);
    struct tm tm;
    localtime_r(&t, &tm);
    if (tm.tm_year < 80) tm.tm_year = 80, tm.tm_mon = 0, tm.tm_mday = 1, tm.tm_hour = tm.tm_min = tm.tm_sec = 0;
    put_le16(date_time_at, (uint32_t)(tm.tm_hour << 11 | tm.tm_min << 5 | tm.tm_sec / 2));
    put_le16(date_time_at + 2, (uint32_t)((tm.tm_year - 80) << 9 | (tm.tm_mon + 1) << 5 | tm.tm_mday));
}

static int64_t unix_time(uint32_t date, uint32_t tim) {
    if (!date) return 0;
    struct tm tm = { 0 };
    tm.tm_year = (int)(date >> 9) + 80;
    tm.tm_mon = (int)((date >> 5) & 15) - 1;
    tm.tm_mday = (int)(date & 31);
    tm.tm_hour = (int)(tim >> 11);
    tm.tm_min = (int)((tim >> 5) & 63);
    tm.tm_sec = (int)(tim & 31) * 2;
    tm.tm_isdst = -1;
    return (int64_t)mktime(&tm) * 1000000000;
}

static int64_t now_ns(void) { return rx_clock_get_utc(); }

// ---- directories ----
//
// A directory is named by its first cluster; 0 is the FAT12/16 root (a
// fixed region). Entries are numbered from 0.

static uint32_t root_dir(void) { return fat_type == 32 ? root_cluster : 0; }

// The 32-byte entry `index` of directory `dir`; with `extend`, a
// directory that is too short grows by a zeroed cluster. -ENOENT past the
// end, -ENOSPC if it cannot grow.
static int dir_entry(uint32_t dir, uint32_t index, int extend, uint8_t **e, struct csec **cs) {
    uint32_t per_sector = ssz / 32;
    uint64_t sector;
    if (dir == 0 && fat_type != 32) {
        if (index >= root_sectors * per_sector) return extend ? -ENOSPC : -ENOENT;
        sector = root_start + index / per_sector;
    } else {
        uint32_t per_cluster = csz / 32, k = index / per_cluster, c = dir, i = 0;
        if (memo_dir == dir && memo_k <= k && memo_c) {
            c = memo_c;
            i = memo_k;
        }
        for (; i < k; i++) {
            uint32_t next = fat_get(c);
            if (is_eoc(next) || !valid_cluster(next)) {
                if (!extend) return -ENOENT;
                if (read_only) return -EROFS;
                next = alloc_cluster(c);
                if (!next) return -ENOSPC;
                if (zero_cluster(next) < 0) return -EIO;
            }
            c = next;
        }
        memo_dir = dir;
        memo_k = k;
        memo_c = c;
        sector = cluster_sector(c) + (index % per_cluster) / per_sector;
    }
    struct csec *s = get_sector(sector);
    if (!s) return -EIO;
    *e = s->data + (index % per_sector) * 32;
    *cs = s;
    return 0;
}

struct dinfo {
    char name[256];         // UTF-8
    char sfn[13];           // "NAME.EXT"
    uint32_t first, sfn_index, nent;
    uint8_t attr, ntcase;
    uint32_t cluster, size;
    uint32_t wdate, wtime;
};

static uint8_t sfn_checksum(const uint8_t *n) {
    uint8_t s = 0;
    for (int i = 0; i < 11; i++) s = (uint8_t)(((s & 1) << 7) + (s >> 1) + n[i]);
    return s;
}

static size_t utf8_put(char *out, size_t cap, size_t at, uint32_t c) {
    char b[4];
    size_t n;
    if (c < 0x80) { b[0] = (char)c; n = 1; }
    else if (c < 0x800) { b[0] = (char)(0xC0 | c >> 6); b[1] = (char)(0x80 | (c & 63)); n = 2; }
    else if (c < 0x10000) { b[0] = (char)(0xE0 | c >> 12); b[1] = (char)(0x80 | (c >> 6 & 63)); b[2] = (char)(0x80 | (c & 63)); n = 3; }
    else { b[0] = (char)(0xF0 | c >> 18); b[1] = (char)(0x80 | (c >> 12 & 63)); b[2] = (char)(0x80 | (c >> 6 & 63)); b[3] = (char)(0x80 | (c & 63)); n = 4; }
    if (at + n >= cap) return at;
    memcpy(out + at, b, n);
    return at + n;
}

// UTF-8 to UTF-16; returns the length in UTF-16 units, -1 if invalid
static int utf16_of(const char *s, size_t len, uint16_t *out, int cap) {
    const unsigned char *p = (const unsigned char *)s, *end = p + len;
    int n = 0;
    while (p < end) {
        uint32_t c = *p++;
        if (c >= 0xF0 && end - p >= 3) { c = (c & 7) << 18 | (p[0] & 63u) << 12 | (p[1] & 63u) << 6 | (p[2] & 63u); p += 3; }
        else if (c >= 0xE0 && end - p >= 2) { c = (c & 15) << 12 | (p[0] & 63u) << 6 | (p[1] & 63u); p += 2; }
        else if (c >= 0xC0 && end - p >= 1) { c = (c & 31) << 6 | (p[0] & 63u); p += 1; }
        else if (c >= 0x80) return -1;
        if (c >= 0x10000) {
            if (n + 2 > cap) return -1;
            c -= 0x10000;
            out[n++] = (uint16_t)(0xD800 + (c >> 10));
            out[n++] = (uint16_t)(0xDC00 + (c & 0x3FF));
        } else {
            if (n + 1 > cap) return -1;
            out[n++] = (uint16_t)c;
        }
    }
    return n;
}

static void format_sfn(const uint8_t *e, char *out, int ntcase) {
    int n = 0;
    for (int i = 0; i < 8 && e[i] != ' '; i++) {
        char c = (char)(i == 0 && e[0] == 0x05 ? 0xE5 : e[i]);
        out[n++] = (ntcase & 0x08) && c >= 'A' && c <= 'Z' ? (char)(c + 32) : c;
    }
    if (e[8] != ' ') {
        out[n++] = '.';
        for (int i = 8; i < 11 && e[i] != ' '; i++) out[n++] = (ntcase & 0x10) && e[i] >= 'A' && e[i] <= 'Z' ? (char)(e[i] + 32) : (char)e[i];
    }
    out[n] = 0;
}

// The next file entry at or after *index: 1 (with *d filled, *index moved
// past it), 0 at the end, negative on error. Deleted entries, volume
// labels and orphaned long-name parts are skipped.
static int dir_next(uint32_t dir, uint32_t *index, struct dinfo *d) {
    uint16_t lfn[20 * 13 + 1];
    int lfn_count = 0, lfn_next = 0;
    uint8_t lfn_sum = 0;
    uint32_t lfn_first = 0;
    for (;;) {
        uint8_t *e;
        struct csec *s;
        int r = dir_entry(dir, *index, 0, &e, &s);
        if (r == -ENOENT) return 0;
        if (r < 0) return r;
        uint32_t i = (*index)++;
        if (e[0] == 0x00) {
            (*index)--;     // stay on the end marker
            return 0;
        }
        if (e[0] == 0xE5) { lfn_count = 0; continue; }
        if ((e[11] & 0x3F) == ATTR_LFN) {
            int ord = e[0] & 0x1F;
            if (e[0] & 0x40) {
                if (ord < 1 || ord > 20) { lfn_count = 0; continue; }
                lfn_count = ord;
                lfn_next = ord;
                lfn_sum = e[13];
                lfn_first = i;
                memset(lfn, 0xFF, sizeof lfn);
            } else if (!lfn_count || ord != lfn_next - 1 || e[13] != lfn_sum) {
                lfn_count = 0;
                continue;
            }
            lfn_next = ord;
            uint16_t *p = lfn + (ord - 1) * 13;
            for (int k = 0; k < 5; k++) p[k] = (uint16_t)le16(e + 1 + 2 * k);
            for (int k = 0; k < 6; k++) p[5 + k] = (uint16_t)le16(e + 14 + 2 * k);
            for (int k = 0; k < 2; k++) p[11 + k] = (uint16_t)le16(e + 28 + 2 * k);
            continue;
        }
        if (e[11] & ATTR_VOLUME) { lfn_count = 0; continue; }
        memset(d, 0, sizeof *d);
        d->attr = e[11];
        d->ntcase = e[12];
        d->cluster = le16(e + 26) | (fat_type == 32 ? le16(e + 20) << 16 : 0);
        d->size = le32(e + 28);
        d->wtime = le16(e + 22);
        d->wdate = le16(e + 24);
        d->sfn_index = i;
        format_sfn(e, d->sfn, 0);
        if (lfn_count && lfn_next == 1 && lfn_sum == sfn_checksum(e)) {
            size_t at = 0;
            for (int k = 0; k < lfn_count * 13; k++) {
                uint32_t c = lfn[k];
                if (c == 0 || c == 0xFFFF) break;
                if (c >= 0xD800 && c < 0xDC00 && k + 1 < lfn_count * 13) {
                    c = 0x10000 + ((c - 0xD800) << 10) + (lfn[k + 1] - 0xDC00u);
                    k++;
                }
                at = utf8_put(d->name, sizeof d->name, at, c);
            }
            d->name[at] = 0;
            d->first = lfn_first;
            d->nent = (uint32_t)lfn_count + 1;
        } else {
            format_sfn(e, d->name, d->ntcase);
            d->first = i;
            d->nent = 1;
        }
        return 1;
    }
}

// One UTF-8 character of s (at most `end`), folded to upper case as
// Windows compares names: ASCII, Latin-1, Latin Extended-A, Greek and
// Cyrillic
static uint32_t next_upper(const unsigned char **s, const unsigned char *end) {
    const unsigned char *p = *s;
    uint32_t c = *p++;
    if (c >= 0xE0 && end - p >= 2) { c = (c & 15) << 12 | (p[0] & 63u) << 6 | (p[1] & 63u); p += 2; }
    else if (c >= 0xC0 && end - p >= 1) { c = (c & 31) << 6 | (p[0] & 63u); p += 1; }
    *s = p;
    if (c >= 'a' && c <= 'z') return c - 32;
    if (c >= 0xE0 && c <= 0xFE && c != 0xF7) return c - 32;
    if (c >= 0x100 && c <= 0x17F && c != 0x130 && c != 0x131 && c != 0x138 && c != 0x149 && c != 0x17F) {
        // Pairs: even upper, odd lower, except 0x139-0x148 and 0x179-0x17E (odd upper)
        if ((c >= 0x139 && c <= 0x148) || (c >= 0x179 && c <= 0x17E)) return (c & 1) ? c : c - 1;
        return c & ~1u;
    }
    if (c >= 0x3B1 && c <= 0x3C9 && c != 0x3C2) return c - 32;
    if (c >= 0x430 && c <= 0x44F) return c - 32;
    if (c >= 0x450 && c <= 0x45F) return c - 80;
    return c;
}

static int name_eq(const char *a, size_t alen, const char *b) {
    const unsigned char *p = (const unsigned char *)a, *pe = p + alen;
    const unsigned char *q = (const unsigned char *)b, *qe = q + strlen(b);
    while (p < pe && q < qe) {
        if (next_upper(&p, pe) != next_upper(&q, qe)) return 0;
    }
    return p == pe && q == qe;
}

// Find `name` in `dir` (case-insensitive; long or short name)
static int dir_find(uint32_t dir, const char *name, size_t len, struct dinfo *d) {
    uint32_t i = 0;
    int r;
    while ((r = dir_next(dir, &i, d)) == 1) {
        if (name_eq(name, len, d->name) || name_eq(name, len, d->sfn)) return 0;
    }
    return r < 0 ? r : -ENOENT;
}

// ---- making entries ----

static int sfn_char(unsigned char c) {
    if (c >= 'A' && c <= 'Z') return 1;
    if (c >= '0' && c <= '9') return 1;
    return c > 127 ? 0 : strchr("!#$%&'()-@^_`{}~", c) && c ? 1 : 0;
}

// The 8.3 form of `name` if it is a valid short name (*case gets the NT
// lower-case bits); 0 if a long name is needed
static int exact_sfn(const char *name, size_t len, uint8_t out[11], uint8_t *ntcase) {
    memset(out, ' ', 11);
    *ntcase = 0;
    const char *dot = memchr(name, '.', len);
    size_t blen = dot ? (size_t)(dot - name) : len, elen = dot ? len - blen - 1 : 0;
    if (!blen || blen > 8 || elen > 3 || (dot && (!elen || memchr(dot + 1, '.', elen)))) return 0;
    int lower[2] = { 0, 0 }, upper[2] = { 0, 0 };
    for (size_t i = 0; i < len; i++) {
        if (name + i == dot) continue;
        int part = dot && name + i > dot;
        unsigned char c = (unsigned char)name[i];
        if (c >= 'a' && c <= 'z') { lower[part] = 1; c = (unsigned char)(c - 32); }
        else if (c >= 'A' && c <= 'Z') upper[part] = 1;
        if (!sfn_char(c)) return 0;
        if (part) out[8 + (i - blen - 1)] = c;
        else out[i] = c;
    }
    if ((lower[0] && upper[0]) || (lower[1] && upper[1])) return 0;
    if (out[0] == 0xE5) out[0] = 0x05;
    *ntcase = (uint8_t)((lower[0] ? 0x08 : 0) | (lower[1] ? 0x10 : 0));
    return 1;
}

static int sfn_taken(uint32_t dir, const uint8_t n[11]) {
    uint32_t i = 0;
    for (;;) {
        uint8_t *e;
        struct csec *s;
        if (dir_entry(dir, i++, 0, &e, &s) < 0 || e[0] == 0) return 0;
        if (e[0] != 0xE5 && (e[11] & 0x3F) != ATTR_LFN && !memcmp(e, n, 11)) return 1;
    }
}

// A unique short name NAME~N.EXT for a long name
static int make_sfn(uint32_t dir, const char *name, size_t len, uint8_t out[11]) {
    char base[9] = { 0 }, ext[4] = { 0 };
    const char *dot = 0;
    for (size_t i = 0; i < len; i++) if (name[i] == '.') dot = name + i;
    size_t bl = 0, el = 0;
    for (const char *p = name; p < (dot ? dot : name + len) && bl < 6; p++) {
        unsigned char c = (unsigned char)*p;
        if (c == ' ' || c == '.') continue;
        if (c >= 'a' && c <= 'z') c = (unsigned char)(c - 32);
        base[bl++] = sfn_char(c) ? (char)c : '_';
    }
    if (!bl) base[bl++] = '_';
    if (dot) {
        for (const char *p = dot + 1; p < name + len && el < 3; p++) {
            unsigned char c = (unsigned char)*p;
            if (c == ' ') continue;
            if (c >= 'a' && c <= 'z') c = (unsigned char)(c - 32);
            ext[el++] = sfn_char(c) ? (char)c : '_';
        }
    }
    for (uint32_t n = 1; n < 1000000; n++) {
        char tail[8];
        int tl = snprintf(tail, sizeof tail, "~%u", n);
        size_t keep = bl;
        if (keep + (size_t)tl > 8) keep = 8 - (size_t)tl;
        memset(out, ' ', 11);
        memcpy(out, base, keep);
        memcpy(out + keep, tail, (size_t)tl);
        memcpy(out + 8, ext, el);
        if (!sfn_taken(dir, out)) return 0;
    }
    return -EEXIST;
}

static int valid_long_name(const char *name, size_t len) {
    if (!len || len > 255) return 0;
    if ((len == 1 && name[0] == '.') || (len == 2 && name[0] == '.' && name[1] == '.')) return 0;
    for (size_t i = 0; i < len; i++) {
        unsigned char c = (unsigned char)name[i];
        if (c < 0x20 || strchr("\\/:*?\"<>|", c)) return 0;
    }
    return 1;
}

// Add an entry for `name` to `dir`: long-name entries if needed, then the
// short entry with `attr`, `cluster`, size 0 and the current time. *index
// gets the short entry's number.
static int dir_add(uint32_t dir, const char *name, size_t len, uint8_t attr, uint32_t cluster, uint32_t *index) {
    if (!valid_long_name(name, len)) return -EINVAL;
    uint8_t sfn[11], ntcase = 0;
    uint16_t u16[256];
    int nlfn = 0, ulen = 0;
    if (!exact_sfn(name, len, sfn, &ntcase) || sfn_taken(dir, sfn)) {
        ulen = utf16_of(name, len, u16, 255);
        if (ulen < 1) return -EINVAL;
        int r = make_sfn(dir, name, len, sfn);
        if (r < 0) return r;
        ntcase = 0;
        nlfn = (ulen + 12) / 13;
    }
    // A run of free entries (deleted, or past the end marker)
    uint32_t need = (uint32_t)nlfn + 1, run = 0, start = 0, i = 0;
    for (;; i++) {
        uint8_t *e;
        struct csec *s;
        int r = dir_entry(dir, i, 1, &e, &s);
        if (r < 0) return r;
        if (e[0] == 0x00 || e[0] == 0xE5) {
            if (!run) start = i;
            if (++run == need) break;
        } else {
            run = 0;
        }
    }
    // Entries past an end marker that we use must not leave a gap: a
    // following 0x00 entry still ends the directory
    uint8_t sum = sfn_checksum(sfn);
    for (int k = 0; k < nlfn; k++) {
        uint8_t *e;
        struct csec *s;
        int r = dir_entry(dir, start + (uint32_t)k, 1, &e, &s);
        if (r < 0) return r;
        int ord = nlfn - k;     // highest part first
        memset(e, 0, 32);
        e[0] = (uint8_t)(ord | (k == 0 ? 0x40 : 0));
        e[11] = ATTR_LFN;
        e[13] = sum;
        for (int c = 0; c < 13; c++) {
            int pos = (ord - 1) * 13 + c;
            uint16_t v = pos < ulen ? u16[pos] : pos == ulen ? 0 : 0xFFFF;
            uint8_t *at = c < 5 ? e + 1 + 2 * c : c < 11 ? e + 14 + 2 * (c - 5) : e + 28 + 2 * (c - 11);
            put_le16(at, v);
        }
        s->dirty = 1;
    }
    uint8_t *e;
    struct csec *s;
    int r = dir_entry(dir, start + (uint32_t)nlfn, 1, &e, &s);
    if (r < 0) return r;
    memset(e, 0, 32);
    memcpy(e, sfn, 11);
    e[11] = attr;
    e[12] = ntcase;
    uint8_t t[4];
    fat_time(now_ns(), t);
    memcpy(e + 14, t, 4);   // created
    put_le16(e + 18, le16(t + 2));  // accessed
    memcpy(e + 22, t, 4);   // written
    put_le16(e + 26, cluster & 0xFFFF);
    if (fat_type == 32) put_le16(e + 20, cluster >> 16);
    s->dirty = 1;
    *index = start + (uint32_t)nlfn;
    return 0;
}

// Mark an entry group deleted
static int dir_remove(uint32_t dir, uint32_t first, uint32_t nent) {
    for (uint32_t k = 0; k < nent; k++) {
        uint8_t *e;
        struct csec *s;
        int r = dir_entry(dir, first + k, 0, &e, &s);
        if (r < 0) return r;
        e[0] = 0xE5;
        s->dirty = 1;
    }
    return 0;
}

static int dir_empty(uint32_t dir) {
    uint32_t i = 0;
    struct dinfo d;
    int r;
    while ((r = dir_next(dir, &i, &d)) == 1) {
        if (strcmp(d.sfn, ".") && strcmp(d.sfn, "..")) return 0;
    }
    return r < 0 ? r : 1;
}

// ---- open files ----

struct fnode {
    uint32_t parent;        // directory holding the entry
    uint32_t index;         // the short entry's number (UINT32_MAX: the root)
    uint32_t first, nent;   // the whole entry group (for rename/unlink)
    uint8_t attr;
    uint32_t cluster, size;
    int64_t mtime;
    int refs;
    int deleted;            // unlinked while open: freed on the last close
    uint32_t memo_k, memo_c;    // last cluster looked up (index, cluster)
    struct fnode *next;
};

static struct fnode *fnodes, *root_node;

struct ofd {
    struct fnode *n;
    uint64_t offset;
    int flags;
    uint32_t dir_pos;
    int refs;
};

struct conn {
    rx_handle_t ch;
    struct ofd *o;
};

static struct conn conns[MAX_CONNS];
static int nconns;

static int is_dir(const struct fnode *n) { return (n->attr & ATTR_DIR) != 0; }
static uint32_t dir_id(const struct fnode *n) { return n == root_node ? root_dir() : n->cluster; }

static uint64_t ino_of(const struct fnode *n) {
    if (n == root_node) return 1;
    return ((uint64_t)n->parent << 20) ^ (uint64_t)n->index ^ ((uint64_t)n->index << 40) ^ 2;
}

static struct fnode *node_get(const struct dinfo *d, uint32_t parent) {
    for (struct fnode *n = fnodes; n; n = n->next) {
        if (!n->deleted && n->parent == parent && n->index == d->sfn_index) {
            n->refs++;
            return n;
        }
    }
    struct fnode *n = calloc(1, sizeof *n);
    if (!n) return 0;
    n->parent = parent;
    n->index = d->sfn_index;
    n->first = d->first;
    n->nent = d->nent;
    n->attr = d->attr;
    n->cluster = d->cluster;
    n->size = (d->attr & ATTR_DIR) ? 0 : d->size;
    n->mtime = unix_time(d->wdate, d->wtime);
    n->refs = 1;
    n->next = fnodes;
    fnodes = n;
    return n;
}

static void node_put(struct fnode *n) {
    if (n == root_node || --n->refs > 0) return;
    if (n->deleted && valid_cluster(n->cluster)) free_chain(n->cluster);
    struct fnode **p = &fnodes;
    while (*p && *p != n) p = &(*p)->next;
    if (*p) *p = n->next;
    free(n);
}

// Write a node's size, first cluster and time back to its entry
static int node_store(struct fnode *n) {
    if (n == root_node || n->deleted) return 0;
    uint8_t *e;
    struct csec *s;
    int r = dir_entry(n->parent, n->index, 0, &e, &s);
    if (r < 0) return r;
    put_le32(e + 28, is_dir(n) ? 0 : n->size);
    put_le16(e + 26, n->cluster & 0xFFFF);
    if (fat_type == 32) put_le16(e + 20, n->cluster >> 16);
    fat_time(n->mtime, e + 22);
    put_le16(e + 18, le16(e + 24));
    e[11] = n->attr;
    s->dirty = 1;
    return 0;
}

// Cluster number `k` of a file (allocating up to it with `grow`); 0 if
// there is none
static uint32_t file_cluster(struct fnode *n, uint32_t k, int grow) {
    if (!valid_cluster(n->cluster)) {
        if (!grow) return 0;
        n->cluster = alloc_cluster(0);
        if (!n->cluster) return 0;
        n->memo_k = 0;
        n->memo_c = n->cluster;
    }
    uint32_t c = n->cluster, i = 0;
    if (n->memo_c && n->memo_k <= k && valid_cluster(n->memo_c)) {
        c = n->memo_c;
        i = n->memo_k;
    }
    for (; i < k; i++) {
        uint32_t next = fat_get(c);
        if (is_eoc(next) || !valid_cluster(next)) {
            if (!grow) return 0;
            next = alloc_cluster(c);
            if (!next) return 0;
        }
        c = next;
    }
    n->memo_k = k;
    n->memo_c = c;
    return c;
}

// Read or write `len` bytes of a file at `off` (the clusters must exist
// for a write: file_reserve). Whole sectors go straight to the disk in
// runs of adjacent clusters; partial sectors are read, changed, written.
static int file_io(struct fnode *n, uint64_t off, uint8_t *buf, size_t len, int write) {
    while (len) {
        uint32_t k = (uint32_t)(off / csz), in = (uint32_t)(off % csz);
        uint32_t c = file_cluster(n, k, 0);
        if (!c) return -EIO;
        // Extend the run over following adjacent clusters
        uint32_t run = 1;
        size_t want = in + len;
        while ((uint64_t)run * csz < want && (uint64_t)(run + 1) * csz <= IOBUF) {
            uint32_t next = fat_get(c + run - 1);
            if (next != c + run) break;
            run++;
        }
        uint64_t sector = cluster_sector(c) + in / ssz;
        uint32_t sin = in % ssz;
        size_t avail = (size_t)run * csz - in;
        size_t n_bytes = len < avail ? len : avail;
        if (sin || n_bytes < ssz) {
            // Part of one sector
            size_t m = ssz - sin < n_bytes ? ssz - sin : n_bytes;
            static uint8_t sec[4096];
            int r = sector_io(0, sector, 1, sec);
            if (r < 0) return r;
            if (write) {
                memcpy(sec + sin, buf, m);
                if ((r = sector_io(1, sector, 1, sec)) < 0) return r;
            } else {
                memcpy(buf, sec + sin, m);
            }
            buf += m;
            off += m;
            len -= m;
            continue;
        }
        size_t whole = n_bytes / ssz * ssz;
        uint32_t count = (uint32_t)(whole / ssz);
        int r;
        if (write) {
            memcpy(iobuf, buf, whole);
            r = dev_rw(1, sector * spb, count * spb);
        } else {
            r = dev_rw(0, sector * spb, count * spb);
            if (r == 0) memcpy(buf, iobuf, whole);
        }
        if (r < 0) return r;
        buf += whole;
        off += whole;
        len -= whole;
    }
    return 0;
}

// Clusters in a file's chain
static uint32_t chain_length(struct fnode *n) {
    uint32_t len = 0;
    for (uint32_t c = n->cluster; valid_cluster(c) && len <= nclusters; len++) {
        uint32_t next = fat_get(c);
        if (is_eoc(next) || !valid_cluster(next)) { len++; break; }
        c = next;
    }
    return len;
}

// Cut the chain to what the size needs (after a full disk stopped growth)
static void chain_trim(struct fnode *n) {
    uint32_t keep = (uint32_t)(((uint64_t)n->size + csz - 1) / csz);
    n->memo_k = n->memo_c = 0;
    if (!valid_cluster(n->cluster)) return;
    if (!keep) {
        free_chain(n->cluster);
        n->cluster = 0;
        return;
    }
    uint32_t c = file_cluster(n, keep - 1, 0);
    if (!c) return;
    uint32_t next = fat_get(c);
    if (!is_eoc(next) && valid_cluster(next)) free_chain(next);
    fat_set(c, eoc());
    n->memo_k = n->memo_c = 0;
}

// Make sure clusters exist for [0, end)
static int file_reserve(struct fnode *n, uint64_t end) {
    if (!end) return 0;
    uint32_t last = (uint32_t)((end - 1) / csz);
    return file_cluster(n, last, 1) ? 0 : -ENOSPC;
}

static int file_truncate(struct fnode *n, uint64_t size) {
    if (size > 0xFFFFFFFFull) return -EFBIG;
    if (size > n->size) {
        // FAT has no holes: write zeros
        if (file_reserve(n, size) < 0) {
            chain_trim(n);
            return -ENOSPC;
        }
        static uint8_t zero[32768];
        uint64_t at = n->size;
        while (at < size) {
            size_t m = size - at < sizeof zero ? (size_t)(size - at) : sizeof zero;
            int r = file_io(n, at, zero, m, 1);
            if (r < 0) return r;
            at += m;
        }
    } else if (size < n->size || (size == 0 && valid_cluster(n->cluster))) {
        if (size == 0) {
            if (valid_cluster(n->cluster)) free_chain(n->cluster);
            n->cluster = 0;
        } else {
            uint32_t keep = (uint32_t)((size - 1) / csz);
            uint32_t c = file_cluster(n, keep, 0);
            if (c) {
                uint32_t next = fat_get(c);
                if (!is_eoc(next) && valid_cluster(next)) free_chain(next);
                fat_set(c, eoc());
            }
        }
        n->memo_k = 0;
        n->memo_c = 0;
    }
    n->size = (uint32_t)size;
    n->mtime = now_ns();
    return node_store(n);
}

// ---- paths ----

// Walk `path` from directory node `dir`. With `parent`: stop before the
// last component (*pdir gets the directory, *last the name). Otherwise
// *out gets a referenced node.
static int walk(struct fnode *dir, const char *path, size_t len, struct fnode **out, struct fnode **pdir,
                const char **last, size_t *last_len) {
    struct fnode *cur = dir;
    cur->refs++;
    size_t i = 0;
    while (i < len && path[i] == '/') i++;
    if (i == len) {
        if (pdir) { node_put(cur); return -EEXIST; }
        *out = cur;
        return 0;
    }
    for (;;) {
        size_t j = i;
        while (j < len && path[j] != '/') j++;
        size_t k = j;
        while (k < len && path[k] == '/') k++;
        int final = k == len;
        if (j - i > 255) { node_put(cur); return -ENAMETOOLONG; }
        if (!is_dir(cur)) { node_put(cur); return -ENOTDIR; }
        if (j - i == 1 && path[i] == '.' && !final) { i = k; continue; }
        if (path[i] == '.' && (j - i == 1 || (j - i == 2 && path[i + 1] == '.'))) {
            // "." last, or "..": fsd resolves these before forwarding
            node_put(cur);
            return -EINVAL;
        }
        if (final && pdir) {
            *pdir = cur;
            *last = path + i;
            *last_len = j - i;
            return 0;
        }
        struct dinfo d;
        int r = dir_find(dir_id(cur), path + i, j - i, &d);
        if (r < 0) { node_put(cur); return r; }
        struct fnode *c = node_get(&d, dir_id(cur));
        node_put(cur);
        if (!c) return -ENOMEM;
        if (final) {
            *out = c;
            return 0;
        }
        cur = c;
        i = k;
    }
}

static void attr_of(struct fnode *n, rx_fs_attr_t *a) {
    memset(a, 0, sizeof *a);
    a->type = is_dir(n) ? RX_FS_TYPE_DIR : RX_FS_TYPE_FILE;
    a->mode = is_dir(n) ? 0755 : (n->attr & ATTR_RO) ? 0444 : 0644;
    a->size = is_dir(n) ? 0 : n->size;
    a->ino = ino_of(n);
    a->nlink = is_dir(n) ? 2 : 1;
    a->mtime_ns = n->mtime;
    a->read_only = (uint32_t)read_only;
    a->fsid = serial ? serial : 0x4d44;
}

// ---- connections ----

static int add_conn(rx_handle_t ch, struct ofd *o) {
    if (nconns == MAX_CONNS) {
        rx_handle_close(ch);
        return -EMFILE;
    }
    conns[nconns].ch = ch;
    conns[nconns++].o = o;
    o->refs++;
    return 0;
}

static int flush(void);

static void drop_conn(int i) {
    struct ofd *o = conns[i].o;
    if (write_pending) flush();
    rx_handle_close(conns[i].ch);
    conns[i] = conns[--nconns];
    if (--o->refs == 0) {
        node_put(o->n);
        free(o);
    }
}

// ---- requests ----

static char req[sizeof(rx_fs_hdr_t) + RX_FS_MAX_IO + 8192];
static char rep[sizeof(rx_fs_reply_t) + RX_FS_MAX_IO + 256];

static int do_open(struct ofd *dir, const char *path, size_t plen, uint32_t flags, uint32_t mode, rx_handle_t server,
                   uint32_t *type) {
    struct fnode *n = 0, *pd;
    const char *last;
    size_t ll;
    int r = walk(dir->n, path, plen, &n, 0, 0, 0);
    if (r == -ENOENT && (flags & O_CREAT)) {
        if (read_only) return -EROFS;
        r = walk(dir->n, path, plen, 0, &pd, &last, &ll);
        if (r < 0) return r;
        uint32_t idx;
        r = dir_add(dir_id(pd), last, ll, (mode & 0200) ? ATTR_ARCH : ATTR_ARCH | ATTR_RO, 0, &idx);
        if (r == 0) {
            struct dinfo d;
            uint32_t i = idx;
            // Read the new group back: its first entry is before idx
            uint32_t s = idx;
            while (s > 0) {
                uint8_t *e;
                struct csec *cs;
                if (dir_entry(dir_id(pd), s - 1, 0, &e, &cs) < 0 || (e[11] & 0x3F) != ATTR_LFN || e[0] == 0xE5) break;
                s--;
            }
            i = s;
            r = dir_next(dir_id(pd), &i, &d);
            r = r == 1 ? 0 : -EIO;
            if (r == 0) n = node_get(&d, dir_id(pd));
            if (!n) r = r ? r : -ENOMEM;
        }
        node_put(pd);
        if (r < 0) return r;
    } else if (r < 0) {
        return r;
    } else if ((flags & O_CREAT) && (flags & O_EXCL)) {
        node_put(n);
        return -EEXIST;
    }
    int writing = (flags & O_ACCMODE) != O_RDONLY;
    if (is_dir(n) && writing) { node_put(n); return -EISDIR; }
    if ((flags & O_DIRECTORY) && !is_dir(n)) { node_put(n); return -ENOTDIR; }
    if ((writing || (flags & O_TRUNC)) && read_only) { node_put(n); return -EROFS; }
    if ((flags & O_TRUNC) && writing && !is_dir(n) && n->size) {
        r = file_truncate(n, 0);
        if (r < 0) { node_put(n); return r; }
    }
    struct ofd *o = calloc(1, sizeof *o);
    if (!o) { node_put(n); return -ENOMEM; }
    o->n = n;
    o->flags = (int)flags;
    r = add_conn(server, o);
    if (r < 0) {
        node_put(n);
        free(o);
        return r;
    }
    *type = is_dir(n) ? RX_FS_TYPE_DIR : RX_FS_TYPE_FILE;
    return 0;
}

static int do_read(struct ofd *o, uint32_t count, char *out, size_t *len) {
    struct fnode *n = o->n;
    if (is_dir(n)) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_WRONLY) return -EBADF;
    if (count > RX_FS_MAX_IO) count = RX_FS_MAX_IO;
    uint64_t avail = o->offset < n->size ? n->size - o->offset : 0;
    size_t m = count < avail ? count : (size_t)avail;
    if (m) {
        int r = file_io(n, o->offset, (uint8_t *)out, m, 0);
        if (r < 0) return r;
    }
    o->offset += m;
    *len = m;
    return 0;
}

static int do_write(struct ofd *o, const char *data, size_t k, uint64_t *wrote) {
    struct fnode *n = o->n;
    if (is_dir(n)) return -EISDIR;
    if ((o->flags & O_ACCMODE) == O_RDONLY) return -EBADF;
    if (read_only) return -EROFS;
    if (o->flags & O_APPEND) o->offset = n->size;
    if (o->offset + k > 0xFFFFFFFFull) return -EFBIG;
    if (o->offset > n->size) {
        int r = file_truncate(n, o->offset);   // zero-fill the gap
        if (r < 0) return r;
    }
    int r = file_reserve(n, o->offset + k);
    if (r < 0) {
        // The disk is full: write what fits, or fail with nothing changed
        uint64_t room = (uint64_t)chain_length(n) * csz;
        if (room > o->offset) {
            k = (size_t)(room - o->offset);
        } else {
            chain_trim(n);
            node_store(n);
            return r;
        }
    }
    if (k) {
        r = file_io(n, o->offset, (uint8_t *)data, k, 1);
        if (r < 0) return r;
    }
    o->offset += k;
    if (o->offset > n->size) n->size = (uint32_t)o->offset;
    n->mtime = now_ns();
    n->attr |= ATTR_ARCH;
    *wrote = k;
    return node_store(n);
}

static int do_readdir(struct ofd *o, uint32_t max, char *out, size_t *len) {
    if (!is_dir(o->n)) return -ENOTDIR;
    if (max > RX_FS_MAX_IO) max = RX_FS_MAX_IO;
    size_t used = 0;
    uint32_t dir = dir_id(o->n);
    for (;;) {
        uint32_t at = o->dir_pos;
        struct dinfo d;
        int r = dir_next(dir, &at, &d);
        if (r < 0) return r;
        if (r == 0) break;
        if (!strcmp(d.sfn, ".") || !strcmp(d.sfn, "..")) {
            o->dir_pos = at;
            continue;
        }
        size_t nl = strlen(d.name);
        if (used + 10 + nl > max) break;
        struct fnode tmp = { 0 };
        tmp.parent = dir;
        tmp.index = d.sfn_index;
        uint64_t ino = ino_of(&tmp);
        memcpy(out + used, &ino, 8);
        out[used + 8] = (char)((d.attr & ATTR_DIR) ? RX_FS_TYPE_DIR : RX_FS_TYPE_FILE);
        out[used + 9] = (char)nl;
        memcpy(out + used + 10, d.name, nl);
        used += 10 + nl;
        o->dir_pos = at;
    }
    *len = used;
    return 0;
}

static int do_mkdir(struct ofd *dir, const char *path, size_t plen) {
    if (read_only) return -EROFS;
    struct fnode *pd, *ex;
    const char *last;
    size_t ll;
    int r = walk(dir->n, path, plen, 0, &pd, &last, &ll);
    if (r < 0) return r;
    struct dinfo d;
    if (dir_find(dir_id(pd), last, ll, &d) == 0) { node_put(pd); return -EEXIST; }
    (void)ex;
    uint32_t c = alloc_cluster(0), idx;
    if (!c) { node_put(pd); return -ENOSPC; }
    if (zero_cluster(c) < 0) { free_chain(c); node_put(pd); return -EIO; }
    r = dir_add(dir_id(pd), last, ll, ATTR_DIR, c, &idx);
    if (r < 0) { free_chain(c); node_put(pd); return r; }
    // "." and ".." (the root's cluster number is written as 0)
    uint8_t *e;
    struct csec *s;
    uint8_t t[4];
    fat_time(now_ns(), t);
    for (int k = 0; k < 2; k++) {
        if (dir_entry(c, (uint32_t)k, 1, &e, &s) < 0) break;
        memset(e, ' ', 11);
        e[0] = '.';
        if (k) e[1] = '.';
        e[11] = ATTR_DIR;
        memset(e + 12, 0, 20);
        memcpy(e + 14, t, 4);
        memcpy(e + 22, t, 4);
        uint32_t target = k == 0 ? c : (pd == root_node ? 0 : pd->cluster);
        put_le16(e + 26, target & 0xFFFF);
        if (fat_type == 32) put_le16(e + 20, target >> 16);
        s->dirty = 1;
    }
    node_put(pd);
    return 0;
}

static int do_unlink(struct ofd *dir, const char *path, size_t plen, uint32_t flags) {
    if (read_only) return -EROFS;
    struct fnode *n;
    int r = walk(dir->n, path, plen, &n, 0, 0, 0);
    if (r < 0) return r;
    if (n == root_node) { node_put(n); return -EBUSY; }
    if (flags & RX_FS_UNLINK_DIR) {
        if (!is_dir(n)) { node_put(n); return -ENOTDIR; }
        r = dir_empty(n->cluster);
        if (r <= 0) { node_put(n); return r < 0 ? r : -ENOTEMPTY; }
    } else if (is_dir(n)) {
        node_put(n);
        return -EISDIR;
    }
    r = dir_remove(n->parent, n->first, n->nent);
    if (r == 0) n->deleted = 1;   // the clusters go with the last reference
    node_put(n);
    return r;
}

static int do_rename(struct ofd *dir, const char *a, size_t alen, const char *b, size_t blen) {
    if (read_only) return -EROFS;
    // A directory cannot move into itself
    if (blen > alen && !memcmp(a, b, alen) && b[alen] == '/') return -EINVAL;
    struct fnode *src, *dst_dir, *ex = 0;
    const char *dn;
    size_t dnl;
    int r = walk(dir->n, a, alen, &src, 0, 0, 0);
    if (r < 0) return r;
    if (src == root_node) { node_put(src); return -EBUSY; }
    r = walk(dir->n, b, blen, 0, &dst_dir, &dn, &dnl);
    if (r < 0) { node_put(src); return r; }
    struct dinfo d;
    if (dir_find(dir_id(dst_dir), dn, dnl, &d) == 0) {
        ex = node_get(&d, dir_id(dst_dir));
        if (ex == src) {
            // Same entry: only the letters' case changes (or nothing)
            node_put(ex);
            ex = 0;
            if (!strncmp(d.name, dn, dnl) && strlen(d.name) == dnl) { node_put(src); node_put(dst_dir); return 0; }
        } else if (ex) {
            if (is_dir(ex) && !is_dir(src)) r = -EISDIR;
            else if (!is_dir(ex) && is_dir(src)) r = -ENOTDIR;
            else if (is_dir(ex) && (r = dir_empty(ex->cluster)) <= 0) r = r < 0 ? r : -ENOTEMPTY;
            else r = 0;
            if (r < 0) { node_put(ex); node_put(src); node_put(dst_dir); return r; }
            dir_remove(ex->parent, ex->first, ex->nent);
            ex->deleted = 1;
            node_put(ex);
        }
    }
    // New entry first (same cluster, attributes, size, times), then the old one goes
    uint32_t idx;
    uint8_t *e, keep[32];
    struct csec *s;
    r = dir_entry(src->parent, src->index, 0, &e, &s);
    if (r == 0) memcpy(keep, e, 32);
    if (r == 0) r = dir_add(dir_id(dst_dir), dn, dnl, src->attr, src->cluster, &idx);
    if (r < 0) { node_put(src); node_put(dst_dir); return r; }
    // dir_add may have moved the old entry's sector out of the cache: look again
    if (dir_entry(dir_id(dst_dir), idx, 0, &e, &s) == 0) {
        memcpy(e + 13, keep + 13, 19);   // times, cluster, size: as they were
        s->dirty = 1;
    }
    uint32_t old_parent = src->parent;
    dir_remove(src->parent, src->first, src->nent);
    // Re-read the new group's extent
    uint32_t st = idx;
    while (st > 0) {
        uint8_t *x;
        struct csec *xs;
        if (dir_entry(dir_id(dst_dir), st - 1, 0, &x, &xs) < 0 || (x[11] & 0x3F) != ATTR_LFN || x[0] == 0xE5) break;
        st--;
    }
    src->parent = dir_id(dst_dir);
    src->index = idx;
    src->first = st;
    src->nent = idx - st + 1;
    // A moved directory's ".." points at its new parent
    if (is_dir(src) && old_parent != src->parent && dir_entry(src->cluster, 1, 0, &e, &s) == 0 && e[0] == '.' && e[1] == '.') {
        uint32_t target = dst_dir == root_node ? 0 : dst_dir->cluster;
        put_le16(e + 26, target & 0xFFFF);
        if (fat_type == 32) put_le16(e + 20, target >> 16);
        s->dirty = 1;
    }
    node_put(src);
    node_put(dst_dir);
    return 0;
}

static void statfs_of(rx_fs_statfs_t *s) {
    memset(s, 0, sizeof *s);
    if (free_count == 0xFFFFFFFF) count_free();
    s->block_size = csz;
    s->blocks = nclusters;
    s->blocks_free = free_count;
    s->name_max = 255;
    s->flags = read_only ? RX_FS_MOUNT_RDONLY : 0;
    strcpy(s->type, "vfat");
}

static int shutting_down;

// Handle one request on connection `i`; -1 if it was dropped
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

    switch (hdr.op) {
    case RX_FS_OP_OPEN: {
        if (blen < 8 || !h) { st = -EINVAL; break; }
        uint32_t f[2];
        memcpy(f, body, 8);
        if (!is_dir(o->n)) { st = -ENOTDIR; break; }
        st = do_open(o, body + 8, blen - 8, f[0], f[1], h, &u32);
        h = 0;  // the new connection owns it, or it was closed
        if (st == 0) {
            memcpy(out, &u32, 4);
            olen = 4;
        }
        changed = 1;
        break;
    }
    case RX_FS_OP_CLONE:
        if (!h) { st = -EINVAL; break; }
        st = add_conn(h, o);
        h = 0;
        break;
    case RX_FS_OP_STAT:
        attr_of(o->n, (rx_fs_attr_t *)out);
        olen = sizeof(rx_fs_attr_t);
        break;
    case RX_FS_OP_SYMLINK:
    case RX_FS_OP_LINK:
        st = read_only ? -EROFS : -EPERM;   // FAT has neither
        break;
    case RX_FS_OP_READLINK: {
        struct fnode *n;
        st = walk(o->n, body, blen, &n, 0, 0, 0);
        if (st == 0) {
            node_put(n);
            st = -EINVAL;   // there are no links on FAT
        }
        break;
    }
    case RX_FS_OP_LSTAT_AT:
    case RX_FS_OP_STAT_AT: {
        struct fnode *n;
        st = walk(o->n, body, blen, &n, 0, 0, 0);
        if (st == 0) {
            attr_of(n, (rx_fs_attr_t *)out);
            olen = sizeof(rx_fs_attr_t);
            node_put(n);
        }
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
        int64_t base = u32 == SEEK_SET ? 0 : u32 == SEEK_CUR ? (int64_t)o->offset : u32 == SEEK_END ? (int64_t)o->n->size : -1;
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
        if (is_dir(o->n)) st = -EISDIR;
        else if (read_only) st = -EROFS;
        else if ((o->flags & O_ACCMODE) == O_RDONLY) st = -EBADF;
        else st = file_truncate(o->n, len);
        changed = 1;
        break;
    }
    case RX_FS_OP_SETATTR: {
        rx_fs_setattr_t sa;
        if (blen < sizeof sa) { st = -EINVAL; break; }
        memcpy(&sa, body, sizeof sa);
        if (read_only) { st = -EROFS; break; }
        if (sa.mask & RX_FS_SETATTR_MODE) {
            // FAT keeps one permission bit: read-only
            if (sa.mode & 0222) o->n->attr &= (uint8_t)~ATTR_RO;
            else o->n->attr |= ATTR_RO;
        }
        if (sa.mask & RX_FS_SETATTR_MTIME) o->n->mtime = sa.mtime_ns;
        st = node_store(o->n);
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
        st = do_mkdir(o, body + 4, blen - 4);
        changed = 1;
        break;
    case RX_FS_OP_UNLINK:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        st = do_unlink(o, body + 4, blen - 4, u32);
        changed = 1;
        break;
    case RX_FS_OP_RENAME:
        if (blen < 4) { st = -EINVAL; break; }
        memcpy(&u32, body, 4);
        if (u32 > blen - 4) { st = -EINVAL; break; }
        st = do_rename(o, body + 4, u32, body + 4 + u32, blen - 4 - u32);
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
        // Busy while other connections than this one are open
        if (nconns > 1 && !(u32 & RX_FS_UNMOUNT_FORCE)) { st = -EBUSY; break; }
        // Ends even if the disk is gone (pulled out): fsd then unmounts
        if (flush() < 0) {
            fprintf(stderr, "fatfs: %s: could not write everything out\n", dev_name);
            st = -EIO;
        }
        shutting_down = 1;
        break;
    default:
        st = -ENOSYS;
        break;
    }
    if (changed == 2 && !read_only) {
        // File data: metadata follows within half a second, or at close
        write_pending = 1;
    } else if (changed && !read_only) {
        int f = flush();
        if (st >= 0 && f < 0) st = f;
    }
    if (h) rx_handle_close(h);
    rx_fs_reply_t r = { hdr.txid, st };
    memcpy(rep, &r, sizeof r);
    rx_channel_write(conns[i].ch, 0, rep, (uint32_t)(sizeof r + (st < 0 ? 0 : olen)), 0, 0);
    return 0;
}

// ---- mounting ----

static int mount_fs(void) {
    static uint8_t b[4096];
    // The boot sector, read in device blocks first (the sector size is in it)
    if (dev_rw(0, 0, (512 + dev_bs - 1) / dev_bs) < 0) return -EIO;
    memcpy(b, iobuf, 512);
    if (b[510] != 0x55 || b[511] != 0xAA) return -EINVAL;
    ssz = le16(b + 11);
    if ((ssz != 512 && ssz != 1024 && ssz != 2048 && ssz != 4096) || ssz < dev_bs || ssz % dev_bs) return -EINVAL;
    spb = ssz / dev_bs;
    spc = b[13];
    if (!spc || (spc & (spc - 1))) return -EINVAL;
    csz = ssz * spc;
    if (csz > IOBUF) return -EINVAL;
    uint32_t reserved = le16(b + 14);
    nfats = b[16];
    uint32_t root_entries = le16(b + 17);
    uint32_t total = le16(b + 19) ? le16(b + 19) : le32(b + 32);
    uint32_t fatsz = le16(b + 22) ? le16(b + 22) : le32(b + 36);
    if (!reserved || !nfats || nfats > 4 || !fatsz || !total) return -EINVAL;
    if ((uint64_t)total * spb > dev_blocks) return -EINVAL;
    fat_start = reserved;
    fat_sectors = fatsz;
    root_sectors = (root_entries * 32 + ssz - 1) / ssz;
    root_start = reserved + nfats * fatsz;
    data_start = root_start + root_sectors;
    if (data_start >= total) return -EINVAL;
    nclusters = (total - data_start) / spc;
    fat_type = nclusters < 4085 ? 12 : nclusters < 65525 ? 16 : 32;
    if (fat_type == 32) {
        if (root_entries || le16(b + 22)) return -EINVAL;
        root_cluster = le32(b + 44);
        fsinfo_sector = le16(b + 48);
        serial = le32(b + 67);
        memcpy(label, b + 71, 11);
    } else {
        if (!root_entries) return -EINVAL;
        serial = b[38] == 0x29 ? le32(b + 39) : 0;
        memcpy(label, b + 43, 11);
    }
    // The FAT must have room for every cluster
    uint64_t fat_bytes = (uint64_t)fatsz * ssz, need = fat_type == 32 ? (nclusters + 2) * 4ull : fat_type == 16 ? (nclusters + 2) * 2ull : (nclusters + 2) * 3ull / 2;
    if (fat_bytes < need) return -EINVAL;
    for (int i = 0; i < CACHE; i++) {
        cache[i].sector = UINT64_MAX;
        cache[i].data = malloc(ssz);
        if (!cache[i].data) return -ENOMEM;
    }
    if (fat_type == 32 && fsinfo_sector && fsinfo_sector < reserved) {
        struct csec *c = get_sector(fsinfo_sector);
        if (c && le32(c->data) == 0x41615252 && le32(c->data + 484) == 0x61417272) {
            uint32_t fc = le32(c->data + 488), nf = le32(c->data + 492);
            if (fc <= nclusters) free_count = fc;
            if (valid_cluster(nf)) next_free = nf;
        } else {
            fsinfo_sector = 0;
        }
    } else {
        fsinfo_sector = 0;
    }
    if (free_count == 0xFFFFFFFF) count_free();
    if (fat_type == 32 && !valid_cluster(root_cluster)) return -EINVAL;
    root_node = calloc(1, sizeof *root_node);
    if (!root_node) return -ENOMEM;
    root_node->attr = ATTR_DIR;
    root_node->cluster = root_dir();
    root_node->index = UINT32_MAX;
    root_node->refs = 1;
    // The volume label in the root directory (it is the one Windows shows)
    uint32_t i = 0;
    for (;; i++) {
        uint8_t *e;
        struct csec *s;
        if (dir_entry(root_dir(), i, 0, &e, &s) < 0 || e[0] == 0) break;
        if (e[0] != 0xE5 && (e[11] & 0x3F) != ATTR_LFN && (e[11] & ATTR_VOLUME)) {
            memcpy(label, e, 11);
            root_node->mtime = unix_time(le16(e + 24), le16(e + 22));
            break;
        }
    }
    return 0;
}

int main(int argc, char **argv) {
    int a = 1;
    for (; a < argc && argv[a][0] == '-'; a++) {
        if (!strcmp(argv[a], "-r")) read_only = 1;
    }
    if (a != argc - 1) {
        fprintf(stderr, "usage: fatfs [-r] DEVICE   (started by mount)\n");
        return 2;
    }
    dev_name = argv[a];
    // The time zone, read now: while serving, a file access of our own
    // would wait for fsd, which waits for us (it forwards synchronously)
    if (!getenv("TZ")) {
        char zone[128] = "UTC";
        FILE *f = fopen("/etc/timezone", "r");
        if (f) {
            if (fgets(zone, sizeof zone, f)) zone[strcspn(zone, "\r\n")] = 0;
            fclose(f);
        }
        setenv("TZ", *zone ? zone : "UTC", 1);
    }
    tzset();
    rx_handle_t first = rx_take_startup_handle(RX_PA_USER0);
    if (!first) {
        fprintf(stderr, "fatfs: no connection to serve (mount starts fatfs)\n");
        return 2;
    }
    int r = rx_block_open(dev_name, &blk);
    rx_block_info_t info;
    if (r == 0) r = rx_block_info(blk, &info);
    if (r < 0) {
        fprintf(stderr, "fatfs: %s: %s\n", dev_name, strerror(-r));
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
        fprintf(stderr, "fatfs: out of memory\n");
        return 1;
    }
    iobuf = (uint8_t *)va;
    rx_block_req_t q = { 0 };
    q.op = RX_BLOCK_OP_ATTACH_VMO;
    size_t got = 0;
    r = rx_block_call(blk, &q, 0, 0, dup, &vmoid, sizeof vmoid, &got);
    if (r < 0 || got != 4) {
        fprintf(stderr, "fatfs: %s: cannot attach a buffer: %s\n", dev_name, strerror(r < 0 ? -r : EIO));
        return 1;
    }
    r = mount_fs();
    if (r < 0) {
        fprintf(stderr, "fatfs: %s: not a FAT filesystem%s\n", dev_name, r == -EIO ? " (read error)" : "");
        return 1;
    }
    struct ofd *o = calloc(1, sizeof *o);
    o->n = root_node;
    root_node->refs++;
    o->flags = O_RDONLY | O_DIRECTORY;
    add_conn(first, o);

    static rx_wait_item_t items[MAX_CONNS];
    for (;;) {
        int n = nconns;
        if (n == 0 || (shutting_down && n <= 1)) {
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
            if (items[k].pending & RX_CHANNEL_READABLE) {
                serve(k);
            } else if (items[k].pending & RX_PEER_CLOSED) {
                drop_conn(k);
            }
        }
        if (shutting_down) {
            // After the SHUTDOWN reply: stop at once if forced, else when
            // only fsd's root connection remains (it closes next)
            flush();
            return 0;
        }
    }
}
