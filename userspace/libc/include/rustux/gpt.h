// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Partition tables (step 13, docs/DRIVERS.md "Partitions"): GPT read and
// write (UEFI spec, chapter 5) with the protective MBR, and plain MBR
// tables (read only). The disk is reached through callbacks, so the same
// code runs in a driver (rx_blockdev) and in the gpt tool (over the
// block protocol).
//
// GUIDs are kept in on-disk byte order (the first three fields little
// endian); rx_guid_format/rx_guid_parse use the usual text form.

#ifndef RUSTUX_GPT_H
#define RUSTUX_GPT_H

#include <stddef.h>
#include <stdint.h>

#define RX_GPT_MAX_PARTS 128

// Read or write `count` blocks at `lba`; 0 or a negative errno
typedef int (*rx_disk_read_fn)(void *ctx, uint64_t lba, uint32_t count, void *buf);
typedef int (*rx_disk_write_fn)(void *ctx, uint64_t lba, uint32_t count, const void *buf);

typedef struct {
    uint8_t type[16];       // all zero: an unused entry
    uint8_t guid[16];
    uint64_t first, last;   // blocks, inclusive
    uint64_t attrs;
    char name[72];          // UTF-8 (from the entry's UTF-16LE name)
} rx_gpt_part_t;

typedef struct {
    uint8_t disk_guid[16];
    uint64_t first_usable, last_usable;
    uint32_t entries;       // entry slots in the table (128 when we write one)
    int from_backup;        // read: the primary was damaged, the backup used
    rx_gpt_part_t parts[RX_GPT_MAX_PARTS];  // slot k = partition k + 1
} rx_gpt_t;

typedef struct {
    uint8_t type;           // 0: unused
    uint8_t bootable;
    uint32_t first, count;  // blocks (MBR addresses count logical blocks)
} rx_mbr_part_t;

uint32_t rx_crc32(uint32_t crc, const void *data, size_t len);

// 0 with *gpt filled; -ENOENT: no GPT; -EBADMSG: both copies damaged;
// other negative errnos from the callback
int rx_gpt_read(rx_gpt_t *gpt, uint32_t block_size, uint64_t blocks, rx_disk_read_fn rd, void *ctx);

// Write the protective MBR, the primary and the backup GPT for `gpt`
// (entries 128; first_usable/last_usable are recomputed). 0 or -errno.
int rx_gpt_write(rx_gpt_t *gpt, uint32_t block_size, uint64_t blocks, rx_disk_write_fn wr, void *ctx);

// An empty table with a new disk GUID
void rx_gpt_init(rx_gpt_t *gpt, uint32_t block_size, uint64_t blocks);

// The four primary MBR entries (extended partitions are listed, not
// followed). Returns how many are used; -ENOENT without an MBR
// signature or when the MBR is GPT's protective one.
int rx_mbr_read(rx_mbr_part_t parts[4], uint32_t block_size, rx_disk_read_fn rd, void *ctx);

void rx_guid_format(const uint8_t g[16], char out[37]);
int rx_guid_parse(const char *s, uint8_t g[16]);   // 0 or -EINVAL
void rx_guid_random(uint8_t g[16]);                // version 4
int rx_guid_is_zero(const uint8_t g[16]);

// Partition type: a short name ("efi", "msdata", "linux") and a
// description, from a GUID; "" / "unknown" if not in the table
const char *rx_gpt_type_short(const uint8_t g[16]);
const char *rx_gpt_type_name(const uint8_t g[16]);
// GUID from a short name or a GUID string; 0 or -EINVAL
int rx_gpt_type_parse(const char *s, uint8_t g[16]);
const char *rx_mbr_type_name(uint8_t type);

#endif
