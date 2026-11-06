// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fsck.fat: check a FAT filesystem (step 14).
//
//   fsck.fat [-a] [-v] DEV
//
// Checks the boot sector, that the FAT copies agree, every directory and
// cluster chain (clusters in range, no loops, no cluster used twice, file
// sizes matching their chains, "." and ".." entries), long-name entries,
// clusters in use that no file owns (lost), and FSInfo's free count. It
// changes nothing, except with -a: then a wrong FSInfo free count is
// corrected. Run it on a filesystem that is not mounted.
// Exit status: 0 no errors, 1 errors corrected, 4 errors left.

#include <errno.h>
#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static rx_handle_t ch;
static uint32_t dev_bs, ssz, spb, spc, csz, nfats, fatsz, reserved, root_secs, root_start, data_start, nclusters;
static uint32_t root_cluster, fsinfo;
static int type, verbose, errors, warnings;
static uint8_t *fat;          // the first FAT
static uint8_t *owner;        // per cluster: 0 free/unseen, 1 seen
static const char *dev;

static uint32_t le16(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8; }
static uint32_t le32(const uint8_t *p) { return le16(p) | le16(p + 2) << 16; }

static void err(const char *path, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
#include <stdarg.h>
static void err(const char *path, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    printf("%s: ", path && *path ? path : "/");
    vprintf(fmt, ap);
    printf("\n");
    va_end(ap);
    errors++;
}

static int rd(uint64_t sector, uint32_t count, void *buf) {
    int r = rx_block_read(ch, dev_bs, sector * spb, count * spb, buf);
    if (r < 0) printf("%s: read error at sector %llu: %s\n", dev, (unsigned long long)sector, strerror(-r));
    return r;
}

static uint32_t fat_get(const uint8_t *f, uint32_t c) {
    if (type == 32) return le32(f + c * 4) & 0x0FFFFFFF;
    if (type == 16) return le16(f + c * 2);
    uint32_t v = le16(f + c + c / 2);
    return c & 1 ? v >> 4 : v & 0xFFF;
}

static int is_eoc(uint32_t v) { return v >= (type == 32 ? 0x0FFFFFF8u : type == 16 ? 0xFFF8u : 0xFF8u); }
static int is_bad(uint32_t v) { return v == (type == 32 ? 0x0FFFFFF7u : type == 16 ? 0xFFF7u : 0xFF7u); }
static int valid(uint32_t c) { return c >= 2 && c < nclusters + 2; }

// Follow a chain, marking its clusters; the number of clusters (0 if
// the chain is broken: reported)
static uint32_t chain(const char *path, uint32_t c, uint32_t **list) {
    uint32_t n = 0, cap = 0;
    if (list) *list = 0;
    while (1) {
        if (!valid(c)) {
            err(path, "cluster %u is out of range", c);
            return 0;
        }
        if (owner[c]) {
            err(path, "cluster %u is used twice (or the chain loops)", c);
            return 0;
        }
        owner[c] = 1;
        if (list) {
            if (n == cap) {
                cap = cap ? cap * 2 : 16;
                *list = realloc(*list, cap * sizeof **list);
            }
            (*list)[n] = c;
        }
        n++;
        uint32_t next = fat_get(fat, c);
        if (is_eoc(next)) return n;
        if (next == 0) {
            err(path, "the chain ends in a free cluster (after %u)", c);
            return 0;
        }
        if (is_bad(next)) {
            err(path, "the chain runs into a bad cluster (after %u)", c);
            return 0;
        }
        c = next;
    }
}

static uint8_t sfn_sum(const uint8_t *n) {
    uint8_t s = 0;
    for (int i = 0; i < 11; i++) s = (uint8_t)(((s & 1) << 7) + (s >> 1) + n[i]);
    return s;
}

static int files, dirs;

static void check_dir(const char *path, uint32_t first, uint32_t parent, int depth);

// The entries of a directory, as one buffer
static uint8_t *dir_data(const char *path, uint32_t first, uint32_t *bytes) {
    *bytes = 0;
    if (first == 0) {
        uint8_t *b = malloc((size_t)root_secs * ssz);
        if (!b || rd(root_start, root_secs, b) < 0) { free(b); return 0; }
        *bytes = root_secs * ssz;
        return b;
    }
    uint32_t *list, n = chain(path, first, &list);
    if (!n) { free(list); return 0; }
    uint8_t *b = malloc((size_t)n * csz);
    for (uint32_t i = 0; b && i < n; i++) {
        if (rd(data_start + (uint64_t)(list[i] - 2) * spc, spc, b + (size_t)i * csz) < 0) { free(b); b = 0; }
    }
    free(list);
    if (b) *bytes = n * csz;
    return b;
}

static void check_dir(const char *path, uint32_t first, uint32_t parent, int depth) {
    if (depth > 64) {
        err(path, "directories nested too deep (a loop?)");
        return;
    }
    uint32_t bytes;
    uint8_t *b = dir_data(path, first, &bytes);
    if (!b) return;
    dirs++;
    int lfn_left = 0, lfn_sum = 0;
    for (uint32_t off = 0; off < bytes; off += 32) {
        uint8_t *e = b + off;
        if (e[0] == 0) break;
        if (e[0] == 0xE5) { lfn_left = 0; continue; }
        if ((e[11] & 0x3F) == 0x0F) {
            int ord = e[0] & 0x1F;
            if (e[0] & 0x40) {
                if (lfn_left) { printf("%s: an unfinished long name\n", path); warnings++; }
                lfn_left = ord;
                lfn_sum = e[13];
            } else if (ord != lfn_left - 1 || e[13] != lfn_sum) {
                printf("%s: long-name entries out of order\n", path);
                warnings++;
                lfn_left = 0;
                continue;
            }
            lfn_left = ord;
            continue;
        }
        if (lfn_left) {
            if (lfn_left != 1 || lfn_sum != sfn_sum(e)) { printf("%s: a long name that belongs to no file\n", path); warnings++; }
            lfn_left = 0;
        }
        if (e[11] & 0x08) continue;   // the volume label
        char name[16];
        int k = 0;
        for (int i = 0; i < 8 && e[i] != ' '; i++) name[k++] = (char)(i == 0 && e[0] == 5 ? 0xE5 : e[i]);
        if (e[8] != ' ') {
            name[k++] = '.';
            for (int i = 8; i < 11 && e[i] != ' '; i++) name[k++] = (char)e[i];
        }
        name[k] = 0;
        uint32_t c = le16(e + 26) | (type == 32 ? le16(e + 20) << 16 : 0), size = le32(e + 28);
        char sub[1024];
        snprintf(sub, sizeof sub, "%s/%s", strcmp(path, "/") ? path : "", name);
        if (!strcmp(name, ".") || !strcmp(name, "..")) {
            if (first == 0 || (first == root_cluster && type == 32)) {
                err(path, "a \"%s\" entry in the root directory", name);
                continue;
            }
            uint32_t want = !strcmp(name, ".") ? first : (parent == root_cluster && type == 32 ? 0 : parent);
            if (c != want && !(name[1] && c == parent)) err(path, "\"%s\" points at cluster %u, not %u", name, c, want);
            continue;
        }
        for (int i = 0; i < 11; i++) {
            if (e[i] < 0x20 && !(i == 0 && e[i] == 5)) { err(sub, "a control character in the short name"); break; }
        }
        if (e[11] & 0x10) {
            if (!c) { err(sub, "a directory without clusters"); continue; }
            if (verbose) printf("  %s/\n", sub);
            check_dir(sub, c, first == 0 ? (type == 32 ? root_cluster : 0) : first, depth + 1);
        } else {
            files++;
            if (verbose) printf("  %s (%u bytes)\n", sub, size);
            if (!c) {
                if (size) err(sub, "%u bytes but no clusters", size);
                continue;
            }
            uint32_t n = chain(sub, c, 0);
            if (!n) continue;
            uint32_t need = (uint32_t)(((uint64_t)size + csz - 1) / csz);
            if (n != need) err(sub, "%u bytes need %u clusters, the chain has %u", size, need, n);
        }
    }
    free(b);
}

int main(int argc, char **argv) {
    int fix = 0, c;
    while ((c = getopt(argc, argv, "anvy")) != -1) {
        if (c == 'a' || c == 'y') fix = 1;
        else if (c == 'v') verbose = 1;
        else if (c != 'n') {
            fprintf(stderr, "usage: fsck.fat [-a] [-v] DEV\n");
            return 16;
        }
    }
    if (optind != argc - 1) {
        fprintf(stderr, "usage: fsck.fat [-a] [-v] DEV\n");
        return 16;
    }
    dev = argv[optind];
    if (!strncmp(dev, "/dev/", 5)) dev += 5;
    rx_block_info_t info;
    int r = rx_block_open(dev, &ch);
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "fsck.fat: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 8;
    }
    dev_bs = info.block_size;
    uint8_t *b = malloc(4096 > dev_bs ? 4096 : dev_bs);
    if (rx_block_read(ch, dev_bs, 0, (512 + dev_bs - 1) / dev_bs, b) < 0) {
        fprintf(stderr, "fsck.fat: %s: cannot read the boot sector\n", dev);
        return 8;
    }
    ssz = le16(b + 11);
    spc = b[13];
    reserved = le16(b + 14);
    nfats = b[16];
    uint32_t root_entries = le16(b + 17), total = le16(b + 19) ? le16(b + 19) : le32(b + 32);
    fatsz = le16(b + 22) ? le16(b + 22) : le32(b + 36);
    if (b[510] != 0x55 || b[511] != 0xAA || (ssz != 512 && ssz != 1024 && ssz != 2048 && ssz != 4096) || ssz % dev_bs ||
        !spc || (spc & (spc - 1)) || !reserved || !nfats || !fatsz || !total) {
        printf("%s: not a FAT filesystem (the boot sector is not one)\n", dev);
        return 8;
    }
    spb = ssz / dev_bs;
    csz = ssz * spc;
    root_secs = (root_entries * 32 + ssz - 1) / ssz;
    root_start = reserved + nfats * fatsz;
    data_start = root_start + root_secs;
    nclusters = (total - data_start) / spc;
    type = nclusters < 4085 ? 12 : nclusters < 65525 ? 16 : 32;
    if ((uint64_t)total * ssz > info.block_count * dev_bs) err(0, "the filesystem (%u sectors) is larger than %s", total, dev);
    if (type == 32) {
        root_cluster = le32(b + 44);
        fsinfo = le16(b + 48);
    }
    printf("%s: FAT%d, %u clusters of %u bytes, %u FATs of %u sectors\n", dev, type, nclusters, csz, nfats, fatsz);
    fat = malloc((size_t)fatsz * ssz);
    uint8_t *other = malloc((size_t)fatsz * ssz);
    owner = calloc(nclusters + 2, 1);
    if (!fat || !other || !owner) {
        fprintf(stderr, "fsck.fat: out of memory\n");
        return 8;
    }
    if (rd(reserved, fatsz, fat) < 0) return 8;
    for (uint32_t k = 1; k < nfats; k++) {
        if (rd(reserved + (uint64_t)k * fatsz, fatsz, other) < 0) return 8;
        if (memcmp(fat, other, (size_t)fatsz * ssz)) err(0, "FAT copy %u differs from the first", k + 1);
    }
    if (type == 32) {
        if (!valid(root_cluster)) err(0, "the root directory's cluster %u is out of range", root_cluster);
        else check_dir("/", root_cluster, 0, 0);
    } else {
        check_dir("/", 0, 0, 0);
    }
    // Clusters in use that no file owns
    uint32_t lost = 0, free_n = 0;
    for (uint32_t k = 2; k < nclusters + 2; k++) {
        uint32_t v = fat_get(fat, k);
        if (v == 0) free_n++;
        else if (!owner[k] && !is_bad(v)) lost++;
    }
    if (lost) err(0, "%u clusters are in use but belong to no file (lost)", lost);
    int fixed = 0;
    if (type == 32 && fsinfo && fsinfo < reserved) {
        uint8_t *s = malloc(ssz);
        if (rd(fsinfo, 1, s) == 0 && le32(s) == 0x41615252 && le32(s + 484) == 0x61417272) {
            uint32_t fc = le32(s + 488);
            if (fc != 0xFFFFFFFF && fc != free_n) {
                printf("FSInfo says %u clusters are free; %u are\n", fc, free_n);
                if (fix) {
                    s[488] = (uint8_t)free_n; s[489] = (uint8_t)(free_n >> 8);
                    s[490] = (uint8_t)(free_n >> 16); s[491] = (uint8_t)(free_n >> 24);
                    if (rx_block_write(ch, dev_bs, (uint64_t)fsinfo * spb, spb, s) == 0) {
                        rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
                        printf("  corrected\n");
                        fixed = 1;
                    }
                } else {
                    errors++;
                }
            }
        }
        free(s);
    }
    printf("%s: %d files, %d directories, %u/%u clusters in use%s\n", dev, files, dirs, nclusters - free_n, nclusters,
           errors ? "" : warnings ? " (warnings only)" : ", no errors");
    if (errors) {
        printf("%s: %d error%s\n", dev, errors, errors == 1 ? "" : "s");
        return 4;
    }
    return fixed ? 1 : 0;
}
