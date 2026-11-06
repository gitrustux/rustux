// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Serving a disk (step 13, docs/DRIVERS.md "Disk drivers"): a driver
// supplies read/write/flush for whole blocks; this library publishes the
// disk to devmgr, serves the block protocol (<rustux/block.h>) to every
// client, reads the partition table (GPT, else MBR) and serves each
// partition as a block device of its own (vda1, nvme0n1p2), and re-reads
// the table on RX_BLOCK_OP_RESCAN.
//
//     static int rd(void *ctx, uint64_t block, uint32_t count, void *buf) { ... }
//     rx_blockdev_ops_t ops = { rd, wr, fl };
//     rx_block_info_t info = { .name = "vd@", .block_count = n, .block_size = 512 };
//     rx_blockdev_publish(&ops, 0, &info, max_blocks, 0);   // info.name: "vda"
//     return rx_blockdev_run();
//
// Calls to the ops come one at a time from the thread in rx_blockdev_run.
// `count` never exceeds max_blocks; `buf` is ordinary memory (the driver
// copies to and from its DMA buffers).

#ifndef RUSTUX_BLOCKDEV_H
#define RUSTUX_BLOCKDEV_H

#include <rustux/block.h>
#include <stdint.h>

typedef struct {
    // 0 or a negative errno (EIO)
    int (*read)(void *ctx, uint64_t block, uint32_t count, void *buf);
    // NULL: the device is read-only (RX_BLOCK_FLAG_READONLY is set)
    int (*write)(void *ctx, uint64_t block, uint32_t count, const void *buf);
    // NULL: nothing is cached
    int (*flush)(void *ctx);
} rx_blockdev_ops_t;

#define RX_BLOCKDEV_NO_PARTITIONS 1u   // serve the disk only (RAM disks)

// Publish the disk and its partitions. info->name may hold '#' or '@'
// (devmgr fills in a free number or letter); on success it holds the name
// given. max_blocks: the largest transfer `ops` take (0: no limit of their
// own). 0 or a negative errno.
int rx_blockdev_publish(const rx_blockdev_ops_t *ops, void *ctx, rx_block_info_t *info, uint32_t max_blocks, unsigned flags);

// Serve until devmgr goes away (then 0)
int rx_blockdev_run(void);

#endif
