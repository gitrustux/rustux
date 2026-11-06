// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Helpers for PCI drivers (step 13): the device channel devmgr gives a
// driver (<rustux/device.h>) wrapped in plain calls, and DMA buffers.
// docs/DRIVERS.md "Writing a driver".

#ifndef RUSTUX_DRIVER_H
#define RUSTUX_DRIVER_H

#include <rustux/device.h>
#include <stddef.h>
#include <stdint.h>

// The device channel (startup handle PA_USER0); 0 if not started by devmgr
rx_handle_t rx_drv_device(void);

int rx_drv_info(rx_handle_t dev, rx_pci_info_t *info);
// Configuration space (width 1, 2 or 4); reads return 0xFFFFFFFF on error
uint32_t rx_drv_cfg_read(rx_handle_t dev, uint16_t off, int width);
int rx_drv_cfg_write(rx_handle_t dev, uint16_t off, int width, uint32_t value);
// Offset of the first capability `id` at or after `from` (0: from the
// start), or 0
uint8_t rx_drv_find_cap(rx_handle_t dev, uint8_t id, uint8_t from);

// Map memory BAR `index` (uncached); its registers at the returned
// address, NULL on failure. *bar gets its description if not NULL.
volatile uint8_t *rx_drv_map_bar(rx_handle_t dev, uint32_t index, rx_pci_bar_t *bar);
// An interrupt: mode RX_DEV_IRQ_ANY / MSI / MSIX, vector `index`; the mode
// used in *used. 0 or a negative errno.
int rx_drv_interrupt(rx_handle_t dev, uint32_t mode, uint32_t index, rx_handle_t *irq, uint32_t *used);
int rx_drv_bti(rx_handle_t dev, rx_handle_t *bti);
int rx_drv_bus_master(rx_handle_t dev, int on);

// Wait for the interrupt at most `ms` milliseconds: 1 if it fired (and is
// acknowledged), 0 on time-out, negative on error
int rx_drv_wait_irq(rx_handle_t irq, uint32_t ms);

// Physically contiguous, zeroed memory mapped into the driver and pinned
// for the device: `va` for the driver, `pa` for the device
typedef struct {
    volatile uint8_t *va;
    uint64_t pa;
    size_t size;
    rx_handle_t vmo, pmt;
} rx_dma_buf_t;

// low: below 4 GiB (devices with 32-bit DMA). 0 or a negative errno.
int rx_dma_alloc(rx_handle_t bti, size_t size, int low, rx_dma_buf_t *out);

// Memory barrier between filling DMA memory and telling the device
static inline void rx_dma_barrier(void) { __asm__ volatile("mfence" ::: "memory"); }

#endif
