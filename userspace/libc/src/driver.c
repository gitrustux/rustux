// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Helpers for PCI drivers (<rustux/driver.h>).

#include <errno.h>
#include <rustux/driver.h>
#include <rustux/process.h>
#include <string.h>

rx_handle_t rx_drv_device(void) { return rx_take_startup_handle(RX_PA_USER0); }

int rx_drv_info(rx_handle_t dev, rx_pci_info_t *info) {
    return rx_dev_call(dev, RX_DEV_OP_GET_INFO, 0, 0, 0, info, sizeof *info, 0, 0);
}

uint32_t rx_drv_cfg_read(rx_handle_t dev, uint16_t off, int width) {
    uint16_t f[2] = { off, (uint16_t)width };
    uint32_t v = 0xFFFFFFFFu;
    if (rx_dev_call(dev, RX_DEV_OP_CONFIG_READ, f, sizeof f, 0, &v, 4, 0, 0) < 0) return 0xFFFFFFFFu;
    return v;
}

int rx_drv_cfg_write(rx_handle_t dev, uint16_t off, int width, uint32_t value) {
    uint8_t b[8];
    uint16_t f[2] = { off, (uint16_t)width };
    memcpy(b, f, 4);
    memcpy(b + 4, &value, 4);
    return rx_dev_call(dev, RX_DEV_OP_CONFIG_WRITE, b, sizeof b, 0, 0, 0, 0, 0);
}

uint8_t rx_drv_find_cap(rx_handle_t dev, uint8_t id, uint8_t from) {
    if (!(rx_drv_cfg_read(dev, 0x06, 2) & 0x10)) return 0; // no capability list
    uint8_t ptr = from ? (uint8_t)rx_drv_cfg_read(dev, from + 1, 1) : (uint8_t)rx_drv_cfg_read(dev, 0x34, 1);
    for (int guard = 0; ptr >= 0x40 && guard < 48; guard++) {
        ptr &= 0xFC;
        if ((uint8_t)rx_drv_cfg_read(dev, ptr, 1) == id) return ptr;
        ptr = (uint8_t)rx_drv_cfg_read(dev, ptr + 1, 1);
    }
    return 0;
}

volatile uint8_t *rx_drv_map_bar(rx_handle_t dev, uint32_t index, rx_pci_bar_t *out) {
    rx_pci_bar_t bar;
    rx_handle_t vmo = 0;
    if (rx_dev_call(dev, RX_DEV_OP_GET_BAR, &index, 4, 0, &bar, sizeof bar, 0, &vmo) < 0 || !vmo) return 0;
    if (out) *out = bar;
    if (bar.type != RX_PCI_BAR_MEM || !bar.size) {
        rx_handle_close(vmo);
        return 0;
    }
    uint64_t va = 0, len = (bar.size + (bar.address & 0xFFF) + 0xFFF) & ~0xFFFull;
    rx_status_t s = rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, len, &va);
    rx_handle_close(vmo);
    return s == RX_OK ? (volatile uint8_t *)(va + (bar.address & 0xFFF)) : 0;
}

int rx_drv_interrupt(rx_handle_t dev, uint32_t mode, uint32_t index, rx_handle_t *irq, uint32_t *used) {
    uint32_t m[2] = { mode, index }, u = 0;
    *irq = 0;
    int r = rx_dev_call(dev, RX_DEV_OP_GET_INTERRUPT, m, sizeof m, 0, &u, 4, 0, irq);
    if (used) *used = u;
    return r < 0 ? r : *irq ? 0 : -EIO;
}

int rx_drv_bti(rx_handle_t dev, rx_handle_t *bti) {
    *bti = 0;
    int r = rx_dev_call(dev, RX_DEV_OP_GET_BTI, 0, 0, 0, 0, 0, 0, bti);
    return r < 0 ? r : *bti ? 0 : -EIO;
}

int rx_drv_bus_master(rx_handle_t dev, int on) {
    uint32_t v = on ? 1 : 0;
    return rx_dev_call(dev, RX_DEV_OP_BUS_MASTER, &v, 4, 0, 0, 0, 0, 0);
}

int rx_drv_wait_irq(rx_handle_t irq, uint32_t ms) {
    uint64_t deadline = rx_clock_get_monotonic() + (uint64_t)ms * 1000000u;
    rx_status_t s = rx_interrupt_wait_until(irq, 0, deadline);
    if (s == RX_ERR_TIMED_OUT) return 0;
    if (s != RX_OK) return -EIO;
    rx_interrupt_ack(irq);
    return 1;
}

int rx_dma_alloc(rx_handle_t bti, size_t size, int low, rx_dma_buf_t *out) {
    memset(out, 0, sizeof *out);
    size = (size + 0xFFF) & ~(size_t)0xFFF;
    uint64_t va = 0;
    if (rx_vmo_create_contiguous(bti, size, low ? RX_VMO_CONTIGUOUS_LOW : 0, &out->vmo) != RX_OK) return -ENOMEM;
    if (rx_bti_pin(bti, RX_BTI_PERM_READ | RX_BTI_PERM_WRITE | RX_BTI_CONTIGUOUS, out->vmo, 0, size, &out->pa, &out->pmt) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, out->vmo, 0, size, &va) != RX_OK) {
        rx_handle_close(out->vmo);
        if (out->pmt) rx_handle_close(out->pmt);
        memset(out, 0, sizeof *out);
        return -ENOMEM;
    }
    out->va = (volatile uint8_t *)va;
    out->size = size;
    return 0;
}
