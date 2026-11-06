// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// GPT and MBR partition tables (<rustux/gpt.h>).

#include <errno.h>
#include <rustux/gpt.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>

#define ENTRY_SIZE 128u
#define HEADER_SIZE 92u

// ---- CRC32 (IEEE 802.3, as GPT uses) ----

uint32_t rx_crc32(uint32_t crc, const void *data, size_t len) {
    static uint32_t table[256];
    if (!table[1]) {
        for (uint32_t i = 0; i < 256; i++) {
            uint32_t c = i;
            for (int k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            table[i] = c;
        }
    }
    const uint8_t *p = data;
    crc = ~crc;
    while (len--) crc = table[(crc ^ *p++) & 0xFF] ^ (crc >> 8);
    return ~crc;
}

// ---- little-endian fields ----

static uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static uint64_t le64(const uint8_t *p) { return le32(p) | (uint64_t)le32(p + 4) << 32; }
static void put32(uint8_t *p, uint32_t v) { for (int i = 0; i < 4; i++) p[i] = (uint8_t)(v >> (8 * i)); }
static void put64(uint8_t *p, uint64_t v) { for (int i = 0; i < 8; i++) p[i] = (uint8_t)(v >> (8 * i)); }

// ---- GUIDs ----

int rx_guid_is_zero(const uint8_t g[16]) {
    for (int i = 0; i < 16; i++) if (g[i]) return 0;
    return 1;
}

void rx_guid_format(const uint8_t g[16], char out[37]) {
    static const char hx[] = "0123456789ABCDEF";
    // text order: bytes 3 2 1 0 - 5 4 - 7 6 - 8 9 - 10..15
    static const int order[16] = { 3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15 };
    char *o = out;
    for (int i = 0; i < 16; i++) {
        if (i == 4 || i == 6 || i == 8 || i == 10) *o++ = '-';
        *o++ = hx[g[order[i]] >> 4];
        *o++ = hx[g[order[i]] & 15];
    }
    *o = 0;
}

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

int rx_guid_parse(const char *s, uint8_t g[16]) {
    static const int order[16] = { 3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15 };
    if (strlen(s) != 36) return -EINVAL;
    int i = 0;
    for (const char *p = s; *p; ) {
        if (p - s == 8 || p - s == 13 || p - s == 18 || p - s == 23) {
            if (*p++ != '-') return -EINVAL;
            continue;
        }
        int a = hexval(p[0]), b = hexval(p[1]);
        if (a < 0 || b < 0 || i >= 16) return -EINVAL;
        g[order[i++]] = (uint8_t)(a << 4 | b);
        p += 2;
    }
    return i == 16 ? 0 : -EINVAL;
}

void rx_guid_random(uint8_t g[16]) {
    getrandom(g, 16, 0);
    g[7] = (uint8_t)((g[7] & 0x0F) | 0x40); // version 4 (byte 7: high byte of the third field)
    g[8] = (uint8_t)((g[8] & 0x3F) | 0x80); // variant 1
}

// ---- partition types ----

static const struct { const char *guid, *shrt, *name; } types[] = {
    { "C12A7328-F81F-11D2-BA4B-00A0C93EC93B", "efi", "EFI System" },
    { "21686148-6449-6E6F-744E-656564454649", "bios", "BIOS boot" },
    { "E3C9E316-0B5C-4DB8-817D-F92DF00215AE", "msreserved", "Microsoft reserved" },
    { "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", "msdata", "Microsoft basic data" },
    { "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC", "winre", "Windows recovery environment" },
    { "5808C8AA-7E8F-42E0-85D2-E1E90434CFB3", "msldm", "Microsoft LDM metadata" },
    { "AF9B60A0-1431-4F62-BC68-3311714A69AD", "msldmdata", "Microsoft LDM data" },
    { "0FC63DAF-8483-4772-8E79-3D69D8477DE4", "linux", "Linux filesystem" },
    { "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709", "linuxroot", "Linux root (x86-64)" },
    { "933AC7E1-2EB4-4F13-B844-0E14E2AEF915", "linuxhome", "Linux /home" },
    { "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F", "swap", "Linux swap" },
    { "E6D6D379-F507-44C2-A23C-238F2A3DF928", "lvm", "Linux LVM" },
    { "A19D880F-05FC-4D3B-A006-743F0F84911E", "raid", "Linux RAID" },
    { "BC13C2FF-59E6-4262-A352-B275FD6F7172", "xbootldr", "Linux extended boot" },
    { "48465300-0000-11AA-AA11-00306543ECAC", "hfs", "Apple HFS/HFS+" },
    { "7C3457EF-0000-11AA-AA11-00306543ECAC", "apfs", "Apple APFS" },
    { "516E7CB4-6ECF-11D6-8FF8-00022D09712B", "freebsd", "FreeBSD data" },
};

static int type_index(const uint8_t g[16]) {
    char s[37];
    rx_guid_format(g, s);
    for (size_t i = 0; i < sizeof types / sizeof types[0]; i++) if (!strcmp(types[i].guid, s)) return (int)i;
    return -1;
}

const char *rx_gpt_type_short(const uint8_t g[16]) { int i = type_index(g); return i < 0 ? "" : types[i].shrt; }
const char *rx_gpt_type_name(const uint8_t g[16]) { int i = type_index(g); return i < 0 ? "unknown" : types[i].name; }

int rx_gpt_type_parse(const char *s, uint8_t g[16]) {
    for (size_t i = 0; i < sizeof types / sizeof types[0]; i++) {
        if (!strcmp(types[i].shrt, s)) return rx_guid_parse(types[i].guid, g);
    }
    return rx_guid_parse(s, g);
}

const char *rx_mbr_type_name(uint8_t t) {
    switch (t) {
    case 0x01: return "FAT12";
    case 0x04: case 0x06: case 0x0E: return "FAT16";
    case 0x05: case 0x0F: return "Extended";
    case 0x07: return "NTFS/exFAT";
    case 0x0B: case 0x0C: return "FAT32";
    case 0x27: return "Windows recovery";
    case 0x82: return "Linux swap";
    case 0x83: return "Linux";
    case 0x8E: return "Linux LVM";
    case 0xEE: return "GPT protective";
    case 0xEF: return "EFI System";
    default: return "unknown";
    }
}

// ---- UTF-16LE names ----

static void name_from_utf16(const uint8_t *in, char out[72]) {
    size_t o = 0;
    for (int i = 0; i < 36; i++) {
        uint32_t c = in[2 * i] | (uint32_t)in[2 * i + 1] << 8;
        if (!c) break;
        if (c >= 0xD800 && c < 0xDC00 && i + 1 < 36) { // surrogate pair
            uint32_t d = in[2 * i + 2] | (uint32_t)in[2 * i + 3] << 8;
            if (d >= 0xDC00 && d < 0xE000) { c = 0x10000 + ((c - 0xD800) << 10) + (d - 0xDC00); i++; }
        }
        char b[4];
        size_t n;
        if (c < 0x80) { b[0] = (char)c; n = 1; }
        else if (c < 0x800) { b[0] = (char)(0xC0 | c >> 6); b[1] = (char)(0x80 | (c & 63)); n = 2; }
        else if (c < 0x10000) { b[0] = (char)(0xE0 | c >> 12); b[1] = (char)(0x80 | (c >> 6 & 63)); b[2] = (char)(0x80 | (c & 63)); n = 3; }
        else { b[0] = (char)(0xF0 | c >> 18); b[1] = (char)(0x80 | (c >> 12 & 63)); b[2] = (char)(0x80 | (c >> 6 & 63)); b[3] = (char)(0x80 | (c & 63)); n = 4; }
        if (o + n >= 72) break;
        memcpy(out + o, b, n);
        o += n;
    }
    out[o] = 0;
}

static void name_to_utf16(const char *in, uint8_t *out) {
    memset(out, 0, 72);
    const unsigned char *p = (const unsigned char *)in;
    int i = 0;
    while (*p && i < 36) {
        uint32_t c = *p++;
        if (c >= 0xF0 && p[0] && p[1] && p[2]) { c = (c & 7) << 18 | (p[0] & 63u) << 12 | (p[1] & 63u) << 6 | (p[2] & 63u); p += 3; }
        else if (c >= 0xE0 && p[0] && p[1]) { c = (c & 15) << 12 | (p[0] & 63u) << 6 | (p[1] & 63u); p += 2; }
        else if (c >= 0xC0 && p[0]) { c = (c & 31) << 6 | (p[0] & 63u); p += 1; }
        if (c >= 0x10000) {
            if (i + 2 > 36) break;
            c -= 0x10000;
            uint32_t hi = 0xD800 + (c >> 10), lo = 0xDC00 + (c & 0x3FF);
            out[2 * i] = (uint8_t)hi; out[2 * i + 1] = (uint8_t)(hi >> 8); i++;
            out[2 * i] = (uint8_t)lo; out[2 * i + 1] = (uint8_t)(lo >> 8); i++;
        } else {
            out[2 * i] = (uint8_t)c; out[2 * i + 1] = (uint8_t)(c >> 8); i++;
        }
    }
}

// ---- reading ----

// One header at `lba` with its entries; 0, -EBADMSG (damaged), -ENOENT
// (no signature) or a read error
static int read_copy(rx_gpt_t *gpt, uint32_t bs, uint64_t blocks, uint64_t lba, rx_disk_read_fn rd, void *ctx) {
    uint8_t *h = malloc(bs);
    if (!h) return -ENOMEM;
    int r = rd(ctx, lba, 1, h);
    if (r < 0) { free(h); return r; }
    if (memcmp(h, "EFI PART", 8)) { free(h); return -ENOENT; }
    uint32_t hsize = le32(h + 12), hcrc = le32(h + 16);
    int bad = hsize < HEADER_SIZE || hsize > bs || le64(h + 24) != lba;
    if (!bad) {
        put32(h + 16, 0);
        bad = rx_crc32(0, h, hsize) != hcrc;
    }
    uint64_t entries_lba = le64(h + 72);
    uint32_t count = le32(h + 80), esize = le32(h + 84), ecrc = le32(h + 88);
    gpt->first_usable = le64(h + 40);
    gpt->last_usable = le64(h + 48);
    memcpy(gpt->disk_guid, h + 56, 16);
    free(h);
    if (bad || esize < ENTRY_SIZE || esize > 4096 || esize % 8 || count == 0 || count > 1024 ||
        entries_lba >= blocks || gpt->first_usable > gpt->last_usable || gpt->last_usable >= blocks) return -EBADMSG;
    uint64_t bytes = (uint64_t)count * esize;
    uint32_t nblk = (uint32_t)((bytes + bs - 1) / bs);
    if (entries_lba + nblk > blocks) return -EBADMSG;
    uint8_t *e = malloc((size_t)nblk * bs);
    if (!e) return -ENOMEM;
    r = 0;
    for (uint32_t done = 0; done < nblk && r >= 0; ) {
        uint32_t n = nblk - done < 32 ? nblk - done : 32;
        r = rd(ctx, entries_lba + done, n, e + (size_t)done * bs);
        done += n;
    }
    if (r < 0) { free(e); return r; }
    if (rx_crc32(0, e, (size_t)bytes) != ecrc) { free(e); return -EBADMSG; }
    memset(gpt->parts, 0, sizeof gpt->parts);
    gpt->entries = count < RX_GPT_MAX_PARTS ? count : RX_GPT_MAX_PARTS;
    for (uint32_t i = 0; i < gpt->entries; i++) {
        const uint8_t *p = e + (size_t)i * esize;
        rx_gpt_part_t *q = &gpt->parts[i];
        memcpy(q->type, p, 16);
        if (rx_guid_is_zero(q->type)) continue;
        memcpy(q->guid, p + 16, 16);
        q->first = le64(p + 32);
        q->last = le64(p + 40);
        q->attrs = le64(p + 48);
        name_from_utf16(p + 56, q->name);
        // An entry outside the disk is ignored rather than trusted
        if (q->first > q->last || q->last >= blocks) memset(q->type, 0, 16);
    }
    free(e);
    return 0;
}

int rx_gpt_read(rx_gpt_t *gpt, uint32_t bs, uint64_t blocks, rx_disk_read_fn rd, void *ctx) {
    if (bs < 512 || blocks < 3) return -ENOENT;
    memset(gpt, 0, sizeof *gpt);
    int r = read_copy(gpt, bs, blocks, 1, rd, ctx);
    if (r == 0 || (r != -EBADMSG && r != -ENOENT)) return r;
    int primary = r;
    // The backup header is in the last block
    r = read_copy(gpt, bs, blocks, blocks - 1, rd, ctx);
    if (r == 0) {
        gpt->from_backup = 1;
        return 0;
    }
    if (primary == -ENOENT && r == -ENOENT) return -ENOENT;
    return r == -ENOENT || r == -EBADMSG ? -EBADMSG : r;
}

// ---- writing ----

void rx_gpt_init(rx_gpt_t *gpt, uint32_t bs, uint64_t blocks) {
    memset(gpt, 0, sizeof *gpt);
    rx_guid_random(gpt->disk_guid);
    gpt->entries = RX_GPT_MAX_PARTS;
    uint64_t eblk = (RX_GPT_MAX_PARTS * ENTRY_SIZE + bs - 1) / bs;
    gpt->first_usable = 2 + eblk;
    gpt->last_usable = blocks > eblk + 2 ? blocks - 2 - eblk : 0;
}

static int write_header(rx_gpt_t *gpt, uint32_t bs, uint64_t lba, uint64_t alt, uint64_t entries_lba, uint32_t ecrc,
                        rx_disk_write_fn wr, void *ctx) {
    uint8_t *h = calloc(1, bs);
    if (!h) return -ENOMEM;
    memcpy(h, "EFI PART", 8);
    put32(h + 8, 0x00010000);
    put32(h + 12, HEADER_SIZE);
    put64(h + 24, lba);
    put64(h + 32, alt);
    put64(h + 40, gpt->first_usable);
    put64(h + 48, gpt->last_usable);
    memcpy(h + 56, gpt->disk_guid, 16);
    put64(h + 72, entries_lba);
    put32(h + 80, RX_GPT_MAX_PARTS);
    put32(h + 84, ENTRY_SIZE);
    put32(h + 88, ecrc);
    put32(h + 16, rx_crc32(0, h, HEADER_SIZE));
    int r = wr(ctx, lba, 1, h);
    free(h);
    return r;
}

int rx_gpt_write(rx_gpt_t *gpt, uint32_t bs, uint64_t blocks, rx_disk_write_fn wr, void *ctx) {
    uint32_t eblk = (RX_GPT_MAX_PARTS * ENTRY_SIZE + bs - 1) / bs;
    if (bs < 512 || blocks < 2 * (uint64_t)eblk + 4) return -ENOSPC;
    gpt->entries = RX_GPT_MAX_PARTS;
    gpt->first_usable = 2 + eblk;
    gpt->last_usable = blocks - 2 - eblk;
    for (int i = 0; i < RX_GPT_MAX_PARTS; i++) {
        rx_gpt_part_t *p = &gpt->parts[i];
        if (rx_guid_is_zero(p->type)) continue;
        if (p->first < gpt->first_usable || p->last > gpt->last_usable || p->first > p->last) return -ERANGE;
    }
    uint8_t *e = calloc(eblk, bs);
    if (!e) return -ENOMEM;
    for (int i = 0; i < RX_GPT_MAX_PARTS; i++) {
        rx_gpt_part_t *p = &gpt->parts[i];
        if (rx_guid_is_zero(p->type)) continue;
        uint8_t *q = e + (size_t)i * ENTRY_SIZE;
        if (rx_guid_is_zero(p->guid)) rx_guid_random(p->guid);
        memcpy(q, p->type, 16);
        memcpy(q + 16, p->guid, 16);
        put64(q + 32, p->first);
        put64(q + 40, p->last);
        put64(q + 48, p->attrs);
        name_to_utf16(p->name, q + 56);
    }
    uint32_t ecrc = rx_crc32(0, e, RX_GPT_MAX_PARTS * ENTRY_SIZE);
    // Protective MBR: one 0xEE partition over the whole disk (at most 2^32 - 1 blocks)
    uint8_t *m = calloc(1, bs);
    int r = m ? 0 : -ENOMEM;
    if (!r) {
        uint8_t *pe = m + 446;
        pe[1] = 0x00; pe[2] = 0x02; pe[3] = 0x00;      // CHS of LBA 1
        pe[4] = 0xEE;
        pe[5] = 0xFF; pe[6] = 0xFF; pe[7] = 0xFF;
        put32(pe + 8, 1);
        put32(pe + 12, blocks - 1 > 0xFFFFFFFFull ? 0xFFFFFFFFu : (uint32_t)(blocks - 1));
        m[510] = 0x55; m[511] = 0xAA;
        // Backup first, then the primary: a crash in between leaves one good copy
        uint64_t last = blocks - 1;
        r = wr(ctx, last - eblk, eblk, e);
        if (r >= 0) r = write_header(gpt, bs, last, 1, last - eblk, ecrc, wr, ctx);
        if (r >= 0) r = wr(ctx, 2, eblk, e);
        if (r >= 0) r = write_header(gpt, bs, 1, last, 2, ecrc, wr, ctx);
        if (r >= 0) r = wr(ctx, 0, 1, m);
    }
    free(m);
    free(e);
    return r < 0 ? r : 0;
}

// ---- MBR ----

int rx_mbr_read(rx_mbr_part_t parts[4], uint32_t bs, rx_disk_read_fn rd, void *ctx) {
    uint8_t *m = malloc(bs);
    if (!m) return -ENOMEM;
    int r = rd(ctx, 0, 1, m);
    if (r < 0) { free(m); return r; }
    if (m[510] != 0x55 || m[511] != 0xAA) { free(m); return -ENOENT; }
    int used = 0;
    for (int i = 0; i < 4; i++) {
        const uint8_t *e = m + 446 + 16 * i;
        parts[i].type = e[4];
        parts[i].bootable = e[0] == 0x80;
        parts[i].first = le32(e + 8);
        parts[i].count = le32(e + 12);
        if (parts[i].type == 0xEE) { free(m); return -ENOENT; } // protective: a GPT disk
        // A boot sector (FAT) also ends in 55 AA: status bytes other than
        // 0 and 0x80 mean these are not partition entries
        if (e[0] != 0 && e[0] != 0x80) { free(m); return -ENOENT; }
        if (parts[i].type && parts[i].count) used++;
        else parts[i].type = 0;
    }
    free(m);
    return used ? used : -ENOENT;
}
