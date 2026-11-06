// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The block device protocol (step 12, docs/DRIVERS.md): what a disk
// driver serves and a filesystem uses. A block device is published to
// devmgr (RX_DM_OP_PUBLISH_BLOCK in <rustux/device.h>); clients get a
// connection with RX_DM_OP_CONNECT_BLOCK and send requests on it.
//
// Request: rx_block_req_t, then (WRITE) the data. Reply: rx_block_reply_t,
// then (READ) the data, or (GET_INFO) an rx_block_info_t, or (ATTACH_VMO)
// a u32 vmoid. status is 0 or a negative errno: EINVAL (bad op or field),
// ERANGE (blocks past the end), EROFS (write to a read-only device), EIO.

#ifndef RUSTUX_BLOCK_H
#define RUSTUX_BLOCK_H

#include <stdint.h>

#define RX_BLOCK_FLAG_READONLY  1u
#define RX_BLOCK_FLAG_REMOVABLE 2u

typedef struct {
    char name[32];          // ram0, nvme0n1, ...
    uint64_t block_count;
    uint32_t block_size;    // bytes, a power of two >= 512
    uint32_t flags;         // RX_BLOCK_FLAG_*
    char driver[32];        // the program serving it
    char model[48];         // what the device says it is
} rx_block_info_t;

typedef struct {
    uint32_t txid;
    uint32_t op;            // RX_BLOCK_OP_*
    uint64_t block;         // first block
    uint32_t count;         // blocks
    uint32_t vmoid;         // READ_VMO / WRITE_VMO / DETACH_VMO
    uint64_t vmo_offset;    // bytes into the VMO
} rx_block_req_t;

typedef struct {
    uint32_t txid;
    int32_t status;
} rx_block_reply_t;

// Reply: rx_block_info_t
#define RX_BLOCK_OP_GET_INFO   1
// Reply: count * block_size bytes (at most RX_BLOCK_MAX_IO)
#define RX_BLOCK_OP_READ       2
// Request data: count * block_size bytes (at most RX_BLOCK_MAX_IO)
#define RX_BLOCK_OP_WRITE      3
// Everything written so far is on the medium when this returns
#define RX_BLOCK_OP_FLUSH      4
// Request: one handle, a VMO. Reply: u32 vmoid for READ_VMO/WRITE_VMO,
// which move data between the device and the VMO without copying it
// through the channel (any size)
#define RX_BLOCK_OP_ATTACH_VMO 5
#define RX_BLOCK_OP_READ_VMO   6
#define RX_BLOCK_OP_WRITE_VMO  7
#define RX_BLOCK_OP_DETACH_VMO 8

// Most data in one READ or WRITE through the channel
#define RX_BLOCK_MAX_IO 32768u

#endif
