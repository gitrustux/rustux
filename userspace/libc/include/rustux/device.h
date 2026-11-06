// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The device manager's protocols (step 12, docs/DRIVERS.md).
//
// devmgr finds the PCI devices, starts a driver for each device listed in
// /etc/drivers, and gives the driver one channel: the **device channel**.
// Through it, and only through it, the driver gets its device's
// resources: configuration space, BARs (a VMO for memory, an I/O port
// resource for ports), interrupts (MSI or MSI-X) and a BTI for DMA. The
// driver never holds the root resource.
//
// Other programs talk to devmgr through the service /svc/devmgr: listing
// PCI devices (lspci) and block devices (lsblk), publishing a block device
// (drivers), and connecting to one.
//
// Both protocols: request { u32 txid; u32 op; } then the op's fields;
// reply { u32 txid; i32 status; } then the reply's fields. status is 0 or
// a negative errno.

#ifndef RUSTUX_DEVICE_H
#define RUSTUX_DEVICE_H

#include <rustux/block.h>
#include <stdint.h>

typedef struct {
    uint32_t txid;
    uint32_t op;
} rx_dev_hdr_t;

typedef struct {
    uint32_t txid;
    int32_t status;
} rx_dev_reply_t;

// ---- PCI device description ----

#define RX_PCI_BAR_NONE 0u
#define RX_PCI_BAR_MEM  1u
#define RX_PCI_BAR_IO   2u
#define RX_PCI_BAR_64BIT    1u   // flags
#define RX_PCI_BAR_PREFETCH 2u

typedef struct {
    uint64_t address;   // physical address, or first port
    uint64_t size;      // 0 if not sized (devices in use by the kernel)
    uint32_t type;      // RX_PCI_BAR_*
    uint32_t flags;
} rx_pci_bar_t;

#define RX_PCI_CAP_MSI  0x05u
#define RX_PCI_CAP_PCIE 0x10u
#define RX_PCI_CAP_MSIX 0x11u

typedef struct {
    uint16_t segment;
    uint8_t bus, dev, func, pad0;
    uint16_t vendor, device;
    uint16_t subsystem_vendor, subsystem;
    uint8_t class_code, subclass, prog_if, revision;
    uint8_t header_type;
    uint8_t irq_line, irq_pin;  // legacy INTx: line (firmware's PIC number) and pin (1 = INTA)
    uint8_t msi;                // MSI capability offset (0 if none)
    uint8_t msix;               // MSI-X capability offset (0 if none)
    uint8_t pcie;               // PCI Express capability offset (0 if none)
    uint16_t msix_count;        // MSI-X table size
    uint16_t msi_count;         // MSI vectors the device can request
    uint16_t command, status;
    rx_pci_bar_t bars[6];
    char driver[32];            // program bound to it ("" if none)
    uint64_t driver_koid;       // its process (0 if not running)
    uint32_t restarts;          // times its driver was restarted
    uint32_t flags;             // RX_PCI_FLAG_*
} rx_pci_info_t;

#define RX_PCI_FLAG_KERNEL 1u   // used by the kernel itself (USB keyboard): left alone
#define RX_PCI_FLAG_ECAM   2u   // configuration space through ECAM (else ports 0xCF8/0xCFC)

// ---- the device channel (driver <-> devmgr) ----

// Reply: rx_pci_info_t
#define RX_DEV_OP_GET_INFO     1
// Request: u16 offset, u16 width (1, 2 or 4). Reply: u32 value
#define RX_DEV_OP_CONFIG_READ  2
// Request: u16 offset, u16 width, u32 value. BARs, the expansion ROM and
// the MSI/MSI-X capabilities are devmgr's (EPERM).
#define RX_DEV_OP_CONFIG_WRITE 3
// Request: u32 index. Reply: rx_pci_bar_t and one handle: a VMO over the
// BAR (memory; uncached) or an I/O port resource (rx_ioports_request).
// Turns on memory or I/O decoding.
#define RX_DEV_OP_GET_BAR      4
// Request: u32 mode (RX_DEV_IRQ_*), u32 index. Reply: u32 mode used and
// one handle: an interrupt object. devmgr programs the device (MSI:
// index 0 only; MSI-X: index < msix_count) and turns off INTx.
#define RX_DEV_OP_GET_INTERRUPT 5
// Reply: one handle: a BTI for this device (DMA)
#define RX_DEV_OP_GET_BTI      6
// Request: u32 enable. Bus mastering (needed for DMA and MSI)
#define RX_DEV_OP_BUS_MASTER   7

#define RX_DEV_IRQ_ANY  0u   // MSI-X if the device has it, else MSI
#define RX_DEV_IRQ_MSI  1u
#define RX_DEV_IRQ_MSIX 2u

// ---- /svc/devmgr ----

// Reply: u32 count, then count rx_pci_info_t
#define RX_DM_OP_LIST_PCI      1
// Reply: u32 count, then count rx_block_info_t
#define RX_DM_OP_LIST_BLOCK    2
// Request: device name; one handle: the server end of a new connection,
// passed on to the block device's server (as fsd does for services)
#define RX_DM_OP_CONNECT_BLOCK 3
// Request: rx_block_info_t (name: a prefix such as "ram"); one handle: a
// channel on which connection requests (rx_svc_connect_t) will arrive.
// Reply: rx_block_info_t with the name given (ram0, ram1, ...). The device
// goes away when that channel's other end is closed.
#define RX_DM_OP_PUBLISH_BLOCK 4

#include <rustux/syscalls.h>
#include <stddef.h>

// One request and its reply (both protocols above): sends `req` (len
// bytes, after the header) and handle `h` (0: none; consumed), waits for
// the reply, copies at most `cap` bytes of its fields to `out` (*outlen:
// how many) and its handle to *hout (if any; else 0). Returns the reply's
// status (0 or a negative errno); -EPIPE if the server has gone.
int rx_dev_call(rx_handle_t ch, uint32_t op, const void *req, size_t len, rx_handle_t h,
                void *out, size_t cap, size_t *outlen, rx_handle_t *hout);

// Connect to /svc/devmgr (0 or a negative errno)
int rx_devmgr_connect(rx_handle_t *out);

// A connection to block device `name` (ram0, ...) through devmgr
int rx_block_open(const char *name, rx_handle_t *out);
// One block request: data (WRITE) goes out, data (READ, GET_INFO,
// ATTACH_VMO) comes back into `out`. Returns 0 or a negative errno.
int rx_block_call(rx_handle_t ch, rx_block_req_t *req, const void *data, size_t len, rx_handle_t h,
                  void *out, size_t cap, size_t *outlen);

// Conveniences (step 13). rx_block_list: every block device devmgr knows,
// in *out (free() it); the count or a negative errno. rx_block_read and
// rx_block_write take any count of blocks (split into RX_BLOCK_MAX_IO
// requests). rx_block_simple: a request without data (FLUSH, RESCAN).
int rx_block_list(rx_block_info_t **out);
int rx_block_info(rx_handle_t ch, rx_block_info_t *info);
int rx_block_read(rx_handle_t ch, uint32_t block_size, uint64_t block, uint32_t count, void *buf);
int rx_block_write(rx_handle_t ch, uint32_t block_size, uint64_t block, uint32_t count, const void *buf);
int rx_block_simple(rx_handle_t ch, uint32_t op);

#endif
