// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ramdisk: a block device in memory, served with the block protocol
// (<rustux/blockdev.h>) and published to devmgr (it shows in lsblk as
// ram0, ram1, ...). Runs until killed; the device goes away with it.
// Partition tables written to it (gpt) are read like a disk's.
//
//   ramdisk [-r] [-b BLOCKSIZE] [-n NAME] [-q] SIZE      SIZE: bytes, or with K, M or G
//
// For testing filesystems and partition tools without a disk.

#include <errno.h>
#include <rustux/blockdev.h>
#include <rustux/process.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static uint8_t *disk;
static uint32_t bs;

static uint64_t parse_size(const char *s) {
    char *end;
    uint64_t v = strtoull(s, &end, 10);
    switch (*end) {
    case 'k': case 'K': v <<= 10; end++; break;
    case 'm': case 'M': v <<= 20; end++; break;
    case 'g': case 'G': v <<= 30; end++; break;
    }
    if (*end == 'i' || *end == 'B') end++;
    if (*end == 'B') end++;
    return *end ? 0 : v;
}

static int rd(void *ctx, uint64_t block, uint32_t count, void *buf) {
    (void)ctx;
    memcpy(buf, disk + block * bs, (size_t)count * bs);
    return 0;
}

static int wr(void *ctx, uint64_t block, uint32_t count, const void *buf) {
    (void)ctx;
    memcpy(disk + block * bs, buf, (size_t)count * bs);
    return 0;
}

static int usage(FILE *f, int code) {
    fprintf(f, "usage: ramdisk [-r] [-b BLOCKSIZE] [-n NAME] [-q] SIZE\n"
               "Serve a block device in memory (ram0, ram1, ...) until killed; run it with &.\n"
               "  SIZE          bytes, or with K, M or G (16M)\n"
               "  -b BLOCKSIZE  bytes per block, a power of two from 512 (default 512)\n"
               "  -n NAME       the device's name (# and @ become a free number or letter)\n"
               "  -r            read-only\n"
               "  -q            print nothing\n");
    return code;
}

int main(int argc, char **argv) {
    int ro = 0, quiet = 0;
    const char *size_arg = 0, *name = "ram#";
    bs = 512;
    for (int a = 1; a < argc; a++) {
        if (!strcmp(argv[a], "-r")) ro = 1;
        else if (!strcmp(argv[a], "-q")) quiet = 1;
        else if (!strcmp(argv[a], "-n") && a + 1 < argc) name = argv[++a];
        else if (!strcmp(argv[a], "-b") && a + 1 < argc) bs = (uint32_t)strtoul(argv[++a], 0, 10);
        else if (!strcmp(argv[a], "--help") || !strcmp(argv[a], "-h")) return usage(stdout, 0);
        else if (argv[a][0] != '-' && !size_arg) size_arg = argv[a];
        else return usage(stderr, 2);
    }
    uint64_t size = size_arg ? parse_size(size_arg) : 0;
    if (!size || bs < 512 || (bs & (bs - 1)) || size % bs || size > (1ull << 30)) {
        fprintf(stderr, "ramdisk: SIZE must be a multiple of the block size, at most 1G\n");
        return usage(stderr, 2);
    }
    rx_handle_t vmo;
    uint64_t addr;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK) {
        fprintf(stderr, "ramdisk: not enough memory for %s\n", size_arg);
        return 1;
    }
    disk = (uint8_t *)addr;
    rx_block_info_t info = { 0 };
    snprintf(info.name, sizeof info.name, "%s", name);
    info.block_count = size / bs;
    info.block_size = bs;
    info.flags = ro ? RX_BLOCK_FLAG_READONLY : 0;
    strcpy(info.driver, "ramdisk");
    strcpy(info.model, "RAM disk");
    rx_blockdev_ops_t ops = { rd, ro ? 0 : wr, 0 };
    int r = rx_blockdev_publish(&ops, 0, &info, 0, 0);
    if (r < 0) {
        fprintf(stderr, "ramdisk: cannot publish the device (%s)\n", strerror(-r));
        return 1;
    }
    if (!quiet) printf("%s: %llu blocks of %u bytes%s\n", info.name, (unsigned long long)info.block_count, bs, ro ? ", read-only" : "");
    fflush(stdout);
    return rx_blockdev_run();
}
