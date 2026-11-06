// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// blk: read and write block devices from the shell (step 13). Block
// devices are not files in /dev yet, so this is how scripts reach them.
//
//   blk read NAME [-o OFFSET] [-n BYTES]     bytes to standard output
//   blk write NAME [-o OFFSET]               standard input to the device
//   blk info NAME | rescan NAME | flush NAME
//
// OFFSET and BYTES are bytes (K, M, G suffixes allowed); they need not be
// whole blocks: partial blocks are read, changed and written back.

#include <errno.h>
#include <rustux/device.h>
#include <rustux/gpt.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static rx_handle_t ch;
static rx_block_info_t info;

static int parse(const char *s, uint64_t *out) {
    char *end;
    uint64_t v = strtoull(s, &end, 0);
    switch (*end) {
    case 'k': case 'K': v <<= 10; end++; break;
    case 'm': case 'M': v <<= 20; end++; break;
    case 'g': case 'G': v <<= 30; end++; break;
    }
    if (*end == 'i') end++;
    if (*end == 'B') end++;
    *out = v;
    return *end ? -1 : 0;
}

static int usage(FILE *f, int code) {
    fprintf(f, "usage: blk read NAME [-o OFFSET] [-n BYTES]   the device's bytes to standard output\n"
               "       blk write NAME [-o OFFSET]             standard input to the device\n"
               "       blk info NAME                          size, block size, flags, partition data\n"
               "       blk rescan NAME                        the disk's driver re-reads the partition table\n"
               "       blk flush NAME                         written data to the medium\n"
               "OFFSET and BYTES in bytes (K, M, G allowed); default: from 0, to the end.\n");
    return code;
}

static int fail(const char *what, int r) {
    fprintf(stderr, "blk: %s: %s: %s\n", info.name[0] ? info.name : "?", what, strerror(-r));
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 3) return usage(argc > 1 && !strcmp(argv[1], "--help") ? stdout : stderr, argc > 1 && !strcmp(argv[1], "--help") ? 0 : 2);
    const char *cmd = argv[1], *name = argv[2];
    uint64_t off = 0, len = 0;
    int have_len = 0;
    for (int a = 3; a < argc; a++) {
        if (!strcmp(argv[a], "-o") && a + 1 < argc) { if (parse(argv[++a], &off) < 0) return usage(stderr, 2); }
        else if (!strcmp(argv[a], "-n") && a + 1 < argc) { if (parse(argv[++a], &len) < 0) return usage(stderr, 2); have_len = 1; }
        else return usage(stderr, 2);
    }
    int r = rx_block_open(name, &ch);
    if (r < 0) {
        fprintf(stderr, "blk: %s: %s\n", name, r == -ENOENT ? "no such block device" : strerror(-r));
        return 1;
    }
    if ((r = rx_block_info(ch, &info)) < 0) return fail("info", r);
    uint32_t bs = info.block_size;
    uint64_t size = info.block_count * bs;
    if (!strcmp(cmd, "info")) {
        char g[37];
        printf("name %s\nblocks %llu\nblock_size %u\nbytes %llu\nread_only %d\nremovable %d\ndriver %s\nmodel %s\n", info.name,
               (unsigned long long)info.block_count, bs, (unsigned long long)size, !!(info.flags & RX_BLOCK_FLAG_READONLY),
               !!(info.flags & RX_BLOCK_FLAG_REMOVABLE), info.driver, info.model);
        if (info.flags & RX_BLOCK_FLAG_PARTITION) {
            printf("parent %s\nstart %llu\nnumber %u\ntable %s\n", info.parent, (unsigned long long)info.start, info.number,
                   info.flags & RX_BLOCK_FLAG_GPT ? "gpt" : "dos");
            if (info.flags & RX_BLOCK_FLAG_GPT) {
                rx_guid_format(info.type_guid, g);
                printf("type %s (%s)\n", g, rx_gpt_type_name(info.type_guid));
                rx_guid_format(info.guid, g);
                printf("guid %s\nlabel %s\n", g, info.label);
            } else {
                printf("type 0x%02x (%s)\n", info.type_guid[0], rx_mbr_type_name(info.type_guid[0]));
            }
        } else {
            printf("table %s\n", info.flags & RX_BLOCK_FLAG_GPT ? "gpt" : info.flags & RX_BLOCK_FLAG_MBR ? "dos" : "none");
            if (info.flags & RX_BLOCK_FLAG_GPT) {
                rx_guid_format(info.guid, g);
                printf("guid %s\n", g);
            }
        }
        return 0;
    }
    if (!strcmp(cmd, "rescan")) { r = rx_block_simple(ch, RX_BLOCK_OP_RESCAN); return r < 0 ? fail("rescan", r) : 0; }
    if (!strcmp(cmd, "flush")) { r = rx_block_simple(ch, RX_BLOCK_OP_FLUSH); return r < 0 ? fail("flush", r) : 0; }
    if (off > size) { fprintf(stderr, "blk: %s: offset %llu is past the end (%llu bytes)\n", name, (unsigned long long)off, (unsigned long long)size); return 1; }
    size_t chunk = 64 * 1024;
    uint8_t *buf = malloc(chunk + 2 * bs);
    if (!buf) return fail("memory", -ENOMEM);
    if (!strcmp(cmd, "read")) {
        if (!have_len || len > size - off) len = size - off;
        while (len) {
            uint64_t first = off / bs, skip = off % bs;
            uint64_t want = skip + len < chunk ? skip + len : chunk;
            uint32_t nblk = (uint32_t)((want + bs - 1) / bs);
            if ((r = rx_block_read(ch, bs, first, nblk, buf)) < 0) return fail("read", r);
            uint64_t n = (uint64_t)nblk * bs - skip;
            if (n > len) n = len;
            if (fwrite(buf + skip, 1, n, stdout) != n) return 1;
            off += n;
            len -= n;
        }
        fflush(stdout);
        return 0;
    }
    if (!strcmp(cmd, "write")) {
        for (;;) {
            size_t got = 0;
            ssize_t k;
            while (got < chunk && (k = read(0, buf + bs + got, chunk - got)) > 0) got += (size_t)k;
            if (!got) break;
            if (got > size - off) {
                fprintf(stderr, "blk: %s: the data goes past the end; nothing written beyond it\n", name);
                got = (size_t)(size - off);
                if (!got) return 1;
            }
            // Blocks covering [off, off+got): read the partial ones first
            uint64_t first = off / bs, skip = off % bs;
            uint32_t nblk = (uint32_t)((skip + got + bs - 1) / bs);
            uint8_t *w = malloc((size_t)nblk * bs);
            if (!w) return fail("memory", -ENOMEM);
            if (skip && (r = rx_block_read(ch, bs, first, 1, w)) < 0) return fail("read", r);
            if ((skip + got) % bs && (nblk > 1 || !skip) && (r = rx_block_read(ch, bs, first + nblk - 1, 1, w + (size_t)(nblk - 1) * bs)) < 0)
                return fail("read", r);
            memcpy(w + skip, buf + bs, got);
            if ((r = rx_block_write(ch, bs, first, nblk, w)) < 0) return fail("write", r);
            free(w);
            off += got;
        }
        r = rx_block_simple(ch, RX_BLOCK_OP_FLUSH);
        return r < 0 ? fail("flush", r) : 0;
    }
    return usage(stderr, 2);
}
