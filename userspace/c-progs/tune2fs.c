// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// tune2fs: show (or relabel) an ext2/3/4 filesystem's superblock (step 16).
//
//   tune2fs -l DEV          what the superblock says, as Linux's tune2fs -l
//                           (also dumpe2fs -h)
//   tune2fs -L LABEL DEV    a new label (not while it is mounted)

#include <errno.h>
#include <rustux/device.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "ext4lib.h"

static const struct { uint32_t word, bit; const char *name; } features[] = {
    { 0, COMPAT_HAS_JOURNAL, "has_journal" }, { 0, COMPAT_EXT_ATTR, "ext_attr" }, { 0, COMPAT_RESIZE_INODE, "resize_inode" },
    { 0, COMPAT_DIR_INDEX, "dir_index" }, { 0, COMPAT_DIR_PREALLOC, "dir_prealloc" }, { 0, COMPAT_SPARSE_SUPER2, "sparse_super2" },
    { 0, COMPAT_FAST_COMMIT, "fast_commit" }, { 0, COMPAT_STABLE_INODES, "stable_inodes" }, { 0, COMPAT_ORPHAN_FILE, "orphan_file" },
    { 1, INCOMPAT_FILETYPE, "filetype" }, { 1, INCOMPAT_RECOVER, "needs_recovery" }, { 1, INCOMPAT_JOURNAL_DEV, "journal_dev" },
    { 1, INCOMPAT_META_BG, "meta_bg" }, { 1, INCOMPAT_EXTENTS, "extent" }, { 1, INCOMPAT_64BIT, "64bit" }, { 1, INCOMPAT_MMP, "mmp" },
    { 1, INCOMPAT_FLEX_BG, "flex_bg" }, { 1, INCOMPAT_EA_INODE, "ea_inode" }, { 1, INCOMPAT_CSUM_SEED, "metadata_csum_seed" },
    { 1, INCOMPAT_LARGEDIR, "large_dir" }, { 1, INCOMPAT_INLINE_DATA, "inline_data" }, { 1, 0x10000, "encrypt" }, { 1, 0x20000, "casefold" },
    { 2, RO_SPARSE_SUPER, "sparse_super" }, { 2, RO_LARGE_FILE, "large_file" }, { 2, RO_HUGE_FILE, "huge_file" },
    { 2, RO_GDT_CSUM, "uninit_bg" }, { 2, RO_DIR_NLINK, "dir_nlink" }, { 2, RO_EXTRA_ISIZE, "extra_isize" }, { 2, 0x100, "quota" },
    { 2, 0x200, "bigalloc" }, { 2, RO_METADATA_CSUM, "metadata_csum" }, { 2, 0x1000, "read-only" }, { 2, 0x2000, "project" },
    { 2, 0x8000, "verity" }, { 2, RO_ORPHAN_PRESENT, "orphan_present" },
};

static void when(const char *what, uint32_t t) {
    if (!t) { printf("%-26s n/a\n", what); return; }
    time_t tt = t;
    struct tm *tm = gmtime(&tt);
    char buf[64];
    strftime(buf, sizeof buf, "%a %b %e %H:%M:%S %Y", tm);
    printf("%-26s %s\n", what, buf);
}

static void uuid(const char *what, const uint8_t *u) {
    printf("%-26s %02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x\n", what, u[0], u[1], u[2], u[3], u[4], u[5],
           u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]);
}

int main(int argc, char **argv) {
    int list = 0, c;
    const char *label = 0;
    while ((c = getopt(argc, argv, "lL:")) != -1) {
        if (c == 'l') list = 1;
        else if (c == 'L') label = optarg;
        else { fprintf(stderr, "usage: tune2fs -l DEV | tune2fs -L LABEL DEV\n"); return 2; }
    }
    if (optind != argc - 1 || (!list && !label)) {
        fprintf(stderr, "usage: tune2fs -l DEV | tune2fs -L LABEL DEV\n");
        return 2;
    }
    const char *dev = argv[optind];
    if (!strncmp(dev, "/dev/", 5)) dev += 5;
    rx_handle_t ch;
    rx_block_info_t info;
    int r = rx_block_open(dev, &ch);
    if (r == 0) r = rx_block_info(ch, &info);
    if (r < 0) {
        fprintf(stderr, "tune2fs: %s: %s\n", dev, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    uint32_t dbs = info.block_size;
    uint64_t f0 = 1024 / dbs, n0 = (2048 + dbs - 1) / dbs - f0;
    uint8_t buf[4096], *s;
    if (rx_block_read(ch, dbs, f0, (uint32_t)n0, buf) < 0) {
        fprintf(stderr, "tune2fs: %s: cannot read the superblock\n", dev);
        return 1;
    }
    s = buf + 1024 % dbs;
    if (le16(s + 56) != 0xEF53) {
        fprintf(stderr, "tune2fs: %s: not an ext2/3/4 filesystem\n", dev);
        return 1;
    }
    if (label) {
        if (strlen(label) > 16) { fprintf(stderr, "tune2fs: a label has at most 16 characters\n"); return 2; }
        FILE *m = fopen("/etc/mtab", "r");
        char line[600], want[80];
        snprintf(want, sizeof want, "/dev/%s ", dev);
        while (m && fgets(line, sizeof line, m))
            if (!strncmp(line, want, strlen(want))) { fprintf(stderr, "tune2fs: %s is mounted (umount it first)\n", dev); return 1; }
        if (m) fclose(m);
        memset(s + 120, 0, 16);
        memcpy(s + 120, label, strlen(label));
        if (le32(s + 100) & RO_METADATA_CSUM) put32(s + 1020, x4_sb_csum(s));
        if (rx_block_write(ch, dbs, f0, (uint32_t)n0, buf) < 0 || rx_block_simple(ch, RX_BLOCK_OP_FLUSH) < 0) {
            fprintf(stderr, "tune2fs: %s: cannot write the superblock\n", dev);
            return 1;
        }
        if (!list) return 0;
    }
    uint32_t bs = 1024u << le32(s + 24), w[3] = { le32(s + 92), le32(s + 96), le32(s + 100) };
    char name[17] = { 0 }, mnt[65] = { 0 };
    memcpy(name, s + 120, 16);
    memcpy(mnt, s + 136, 64);
    uint64_t blocks = le32(s + 4) | ((w[1] & INCOMPAT_64BIT) ? (uint64_t)le32(s + 336) << 32 : 0);
    uint64_t rblocks = le32(s + 8) | ((w[1] & INCOMPAT_64BIT) ? (uint64_t)le32(s + 340) << 32 : 0);
    uint64_t fblocks = le32(s + 12) | ((w[1] & INCOMPAT_64BIT) ? (uint64_t)le32(s + 344) << 32 : 0);
    printf("tune2fs (Rustux) %s\n", dev);
    printf("%-26s %s\n", "Filesystem volume name:", *name ? name : "<none>");
    printf("%-26s %s\n", "Last mounted on:", *mnt ? mnt : "<not available>");
    uuid("Filesystem UUID:", s + 104);
    printf("%-26s 0x%04X\n", "Filesystem magic number:", le16(s + 56));
    printf("%-26s %u (%s)\n", "Filesystem revision #:", le32(s + 76), le32(s + 76) ? "dynamic" : "original");
    printf("%-26s", "Filesystem features:");
    for (unsigned i = 0; i < sizeof features / sizeof features[0]; i++)
        if (w[features[i].word] & features[i].bit) printf(" %s", features[i].name);
    printf("\n");
    uint32_t fl = le32(s + 352);
    printf("%-26s %s\n", "Filesystem flags:", fl & 1 ? "signed_directory_hash" : fl & 2 ? "unsigned_directory_hash" : "(none)");
    uint32_t st = le16(s + 58);
    printf("%-26s %s%s\n", "Filesystem state:", st & 1 ? "clean" : "not clean", st & 2 ? " with errors" : "");
    uint32_t eb = le16(s + 60);
    printf("%-26s %s\n", "Errors behavior:", eb == 2 ? "Remount read-only" : eb == 3 ? "Panic" : "Continue");
    printf("%-26s %s\n", "Filesystem OS type:", le32(s + 72) == 0 ? "Linux" : "other");
    printf("%-26s %u\n", "Inode count:", le32(s));
    printf("%-26s %llu\n", "Block count:", (unsigned long long)blocks);
    printf("%-26s %llu\n", "Reserved block count:", (unsigned long long)rblocks);
    printf("%-26s %llu\n", "Free blocks:", (unsigned long long)fblocks);
    printf("%-26s %u\n", "Free inodes:", le32(s + 16));
    printf("%-26s %u\n", "First block:", le32(s + 20));
    if (le16(s + 206)) printf("%-26s %u\n", "Reserved GDT blocks:", le16(s + 206));
    printf("%-26s %u\n", "Block size:", bs);
    if (w[1] & INCOMPAT_64BIT) printf("%-26s %u\n", "Group descriptor size:", le16(s + 254));
    printf("%-26s %u\n", "Blocks per group:", le32(s + 32));
    printf("%-26s %u\n", "Inodes per group:", le32(s + 40));
    uint32_t isz = le32(s + 76) ? le16(s + 88) : 128;
    printf("%-26s %u\n", "Inode blocks per group:", (le32(s + 40) * isz + bs - 1) / bs);
    if (w[1] & INCOMPAT_FLEX_BG) printf("%-26s %u\n", "Flex block group size:", 1u << s[372]);
    when("Filesystem created:", le32(s + 264));
    when("Last mount time:", le32(s + 44));
    when("Last write time:", le32(s + 48));
    printf("%-26s %u\n", "Mount count:", le16(s + 52));
    printf("%-26s %d\n", "Maximum mount count:", (int16_t)le16(s + 54));
    when("Last checked:", le32(s + 64));
    printf("%-26s %u\n", "Reserved blocks uid:", le16(s + 80));
    printf("%-26s %u\n", "Reserved blocks gid:", le16(s + 82));
    printf("%-26s %u\n", "First inode:", le32(s + 84));
    printf("%-26s %u\n", "Inode size:", isz);
    if (isz > 128) {
        printf("%-26s %u\n", "Required extra isize:", le16(s + 348));
        printf("%-26s %u\n", "Desired extra isize:", le16(s + 350));
    }
    if (w[0] & COMPAT_HAS_JOURNAL) printf("%-26s %u\n", "Journal inode:", le32(s + 224));
    if (le32(s + 232)) printf("%-26s %u\n", "First orphan inode:", le32(s + 232));
    static const char *hash[] = { "legacy", "half_md4", "tea" };
    printf("%-26s %s\n", "Default directory hash:", s[252] < 3 ? hash[s[252]] : "unknown");
    uuid("Directory Hash Seed:", s + 236);
    if (w[0] & COMPAT_HAS_JOURNAL) printf("%-26s %s\n", "Journal backup:", s[253] == 1 ? "inode blocks" : "none");
    if (w[2] & RO_METADATA_CSUM) {
        printf("%-26s %s\n", "Checksum type:", s[373] == 1 ? "crc32c" : "unknown");
        printf("%-26s 0x%08x%s\n", "Checksum:", le32(s + 1020), le32(s + 1020) == x4_sb_csum(s) ? "" : " (wrong)");
    }
    if (w[1] & INCOMPAT_CSUM_SEED) printf("%-26s 0x%08x\n", "Checksum seed:", le32(s + 624));
    return 0;
}
