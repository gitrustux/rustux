// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// What a partition holds (step 13): recognises filesystems by their
// signatures, as blkid does. Reads the first 40 KiB only.
//
// Types: vfat, exfat, ntfs, BitLocker, ext2, ext3, ext4, swap, iso9660.

#ifndef RUSTUX_FSPROBE_H
#define RUSTUX_FSPROBE_H

#include <rustux/gpt.h>

typedef struct {
    char type[16];
    char label[64];   // "" if none (or not read: NTFS and exFAT keep it elsewhere)
    char uuid[40];
} rx_fsprobe_t;

// 0 if recognised, -ENOENT if not, or a read error
int rx_fs_probe(rx_disk_read_fn rd, void *ctx, uint32_t block_size, uint64_t blocks, rx_fsprobe_t *out);

// The same for a block device by name (opens it through devmgr)
int rx_block_probe(const char *name, rx_fsprobe_t *out);

#endif
