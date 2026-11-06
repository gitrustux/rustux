// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Filesystem signatures (<rustux/fsprobe.h>).

#include <errno.h>
#include <rustux/fsprobe.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PROBE_BYTES (40u * 1024u)

static uint32_t le16(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8; }
static uint32_t le32(const uint8_t *p) { return le16(p) | le16(p + 2) << 16; }

// Copy a space- or NUL-padded label, trimming the padding
static void label(char *out, size_t cap, const uint8_t *in, size_t len) {
    size_t n = 0;
    while (n < len && in[n]) n++;
    while (n && in[n - 1] == ' ') n--;
    if (n >= cap) n = cap - 1;
    memcpy(out, in, n);
    out[n] = 0;
}

static void uuid_be(char out[40], const uint8_t *u) {
    snprintf(out, 40, "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
             u[0], u[1], u[2], u[3], u[4], u[5], u[6], u[7], u[8], u[9], u[10], u[11], u[12], u[13], u[14], u[15]);
}

int rx_fs_probe(rx_disk_read_fn rd, void *ctx, uint32_t bs, uint64_t blocks, rx_fsprobe_t *out) {
    memset(out, 0, sizeof *out);
    if (!bs) return -EINVAL;
    uint64_t want = (PROBE_BYTES + bs - 1) / bs;
    if (want > blocks) want = blocks;
    if (!want) return -ENOENT;
    uint8_t *b = calloc(1, want * bs > PROBE_BYTES ? want * bs : PROBE_BYTES);
    if (!b) return -ENOMEM;
    int r = rd(ctx, 0, (uint32_t)want, b);
    if (r < 0) {
        free(b);
        return r;
    }
    uint64_t have = want * bs;
    r = 0;
    if (!memcmp(b + 3, "NTFS    ", 8)) {
        strcpy(out->type, "ntfs");
        snprintf(out->uuid, sizeof out->uuid, "%08X%08X", le32(b + 0x4C), le32(b + 0x48));
    } else if (!memcmp(b + 3, "-FVE-FS-", 8)) {
        strcpy(out->type, "BitLocker");
    } else if (!memcmp(b + 3, "EXFAT   ", 8)) {
        strcpy(out->type, "exfat");
        uint32_t id = le32(b + 100);
        snprintf(out->uuid, sizeof out->uuid, "%04X-%04X", id >> 16, id & 0xFFFF);
    } else if (b[510] == 0x55 && b[511] == 0xAA && !memcmp(b + 82, "FAT32   ", 8)) {
        strcpy(out->type, "vfat");
        uint32_t id = le32(b + 67);
        snprintf(out->uuid, sizeof out->uuid, "%04X-%04X", id >> 16, id & 0xFFFF);
        label(out->label, sizeof out->label, b + 71, 11);
    } else if (b[510] == 0x55 && b[511] == 0xAA && (!memcmp(b + 54, "FAT12   ", 8) || !memcmp(b + 54, "FAT16   ", 8) || !memcmp(b + 54, "FAT     ", 8))) {
        strcpy(out->type, "vfat");
        uint32_t id = le32(b + 39);
        snprintf(out->uuid, sizeof out->uuid, "%04X-%04X", id >> 16, id & 0xFFFF);
        label(out->label, sizeof out->label, b + 43, 11);
    } else if (have >= 2048 && le16(b + 1024 + 56) == 0xEF53) {
        const uint8_t *sb = b + 1024;
        uint32_t compat = le32(sb + 0x5C), incompat = le32(sb + 0x60);
        strcpy(out->type, incompat & (0x40 | 0x80 | 0x200) ? "ext4" : compat & 0x4 ? "ext3" : "ext2");
        uuid_be(out->uuid, sb + 0x68);
        label(out->label, sizeof out->label, sb + 0x78, 16);
    } else if (have >= 4096 && (!memcmp(b + 4086, "SWAPSPACE2", 10) || !memcmp(b + 4086, "SWAP-SPACE", 10))) {
        strcpy(out->type, "swap");
        uuid_be(out->uuid, b + 1024 + 12);
        label(out->label, sizeof out->label, b + 1024 + 28, 16);
    } else if (have >= 0x8800 && !memcmp(b + 0x8001, "CD001", 5)) {
        strcpy(out->type, "iso9660");
        label(out->label, sizeof out->label, b + 0x8028, 32);
    } else {
        r = -ENOENT;
    }
    if (r == 0 && !strcmp(out->label, "NO NAME")) out->label[0] = 0;
    free(b);
    return r;
}

// ---- by device name (tools) ----

#include <rustux/device.h>

struct chctx { rx_handle_t ch; uint32_t bs; };

static int ch_read(void *c, uint64_t lba, uint32_t count, void *buf) {
    struct chctx *x = c;
    return rx_block_read(x->ch, x->bs, lba, count, buf);
}

int rx_block_probe(const char *name, rx_fsprobe_t *out) {
    memset(out, 0, sizeof *out);
    rx_handle_t ch;
    int r = rx_block_open(name, &ch);
    if (r < 0) return r;
    rx_block_info_t info;
    r = rx_block_info(ch, &info);
    if (r == 0) {
        struct chctx x = { ch, info.block_size };
        r = rx_fs_probe(ch_read, &x, info.block_size, info.block_count, out);
    }
    rx_handle_close(ch);
    return r;
}
