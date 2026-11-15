// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// display: the display service on the UEFI framebuffer (DESKTOP.md step
// F2, docs/DISPLAY.md; the protocol server is displaysvc.h, shared with
// virtio-gpu since step F8).
//
// devmgr starts it like a driver, with the boot framebuffer it claimed
// from the kernel: the framebuffer VMO (RX_PA_USER0, write-combining), the
// claim on the screen (RX_PA_USER1) and the geometry as arguments:
//
//   display WIDTH HEIGHT STRIDE RED GREEN BLUE OFFSET
//
// (STRIDE in bytes, the colour masks in hex, OFFSET of the first pixel in
// the VMO). It publishes /svc/display (<rustux/display.h>): one owner at a
// time draws into a back buffer in RAM, and PRESENT copies the damaged
// rectangles to the framebuffer, converting to its pixel layout. While no
// one owns the screen, or after Ctrl+Alt+F1, the kernel's text console
// shows; when this program ends the kernel shows it too (the claim
// closes) and devmgr starts it again.

#include "displaysvc.h"

static uint32_t stride, offset;
static uint32_t rshift, gshift, bshift;
static int identity;            // the framebuffer is 0x00RRGGBB already
static volatile uint8_t *fb;    // the framebuffer (its first pixel)
static rx_handle_t claim;

static uint32_t mask_shift(uint32_t m) {
    uint32_t s = 0;
    while (s < 32 && !((m >> s) & 1)) s++;
    return s;
}

// Copy a rectangle of the back buffer to the screen
static void copy_rect(uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    if (x >= dsp_width || y >= dsp_height) return;
    if (w > dsp_width - x) w = dsp_width - x;
    if (h > dsp_height - y) h = dsp_height - y;
    for (uint32_t r = y; r < y + h; r++) {
        const uint32_t *src = dsp_back + (uint64_t)r * dsp_width + x;
        volatile uint32_t *dst = (volatile uint32_t *)(fb + (uint64_t)r * stride) + x;
        if (identity) {
            // Whole rows of 32-bit stores: write-combining merges them
            for (uint32_t i = 0; i < w; i++) dst[i] = src[i];
        } else {
            for (uint32_t i = 0; i < w; i++) {
                uint32_t p = src[i];
                dst[i] = ((p >> 16) & 0xFF) << rshift | ((p >> 8) & 0xFF) << gshift | (p & 0xFF) << bshift;
            }
        }
    }
}

static void gop_present(const rx_display_rect_t *r, int n) {
    if (!n) copy_rect(0, 0, dsp_width, dsp_height);
    for (int i = 0; i < n; i++) copy_rect(r[i].x, r[i].y, r[i].w, r[i].h);
}

static void gop_show(int graphics) {
    rx_display_console(claim, graphics ? RX_DISPLAY_GRAPHICS : RX_DISPLAY_TEXT);
}

// A snapshot of what is on the screen, from the framebuffer itself
static rx_handle_t gop_snapshot(uint32_t *ow, uint32_t *oh) {
    rx_handle_t vmo;
    uint64_t size = (uint64_t)dsp_width * dsp_height * 4;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    uint32_t *row = malloc((size_t)dsp_width * 4);
    if (!row) {
        rx_handle_close(vmo);
        return 0;
    }
    for (uint32_t r = 0; r < dsp_height; r++) {
        volatile const uint32_t *src = (volatile const uint32_t *)(fb + (uint64_t)r * stride);
        for (uint32_t i = 0; i < dsp_width; i++) {
            uint32_t p = src[i];
            row[i] = identity ? p & 0xFFFFFF
                              : ((p >> rshift) & 0xFF) << 16 | ((p >> gshift) & 0xFF) << 8 | ((p >> bshift) & 0xFF);
        }
        rx_vmo_write(vmo, row, (uint64_t)r * dsp_width * 4, (uint64_t)dsp_width * 4);
    }
    free(row);
    *ow = dsp_width;
    *oh = dsp_height;
    return vmo;
}

int main(int argc, char **argv) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    rx_handle_t fb_vmo = rx_take_startup_handle(RX_PA_USER0);
    claim = rx_take_startup_handle(RX_PA_USER1);
    if (argc != 8 || !fb_vmo || !claim) {
        fprintf(stderr, "display: start me from devmgr\n");
        return 2;
    }
    uint32_t width = (uint32_t)strtoul(argv[1], 0, 10), height = (uint32_t)strtoul(argv[2], 0, 10);
    dsp_width = width;
    dsp_height = height;
    stride = (uint32_t)strtoul(argv[3], 0, 10);
    uint32_t red = (uint32_t)strtoul(argv[4], 0, 16), green = (uint32_t)strtoul(argv[5], 0, 16),
             blue = (uint32_t)strtoul(argv[6], 0, 16);
    offset = (uint32_t)strtoul(argv[7], 0, 10);
    rshift = mask_shift(red);
    gshift = mask_shift(green);
    bshift = mask_shift(blue);
    identity = red == 0xFF0000 && green == 0xFF00 && blue == 0xFF;
    uint64_t fbsize = 0, addr = 0;
    rx_vmo_get_size(fb_vmo, &fbsize);
    if (!width || !height || stride < width * 4 || offset + (uint64_t)stride * height > fbsize ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, fb_vmo, 0, fbsize, &addr) != RX_OK) {
        fprintf(stderr, "display: cannot map the framebuffer\n");
        return 3;
    }
    fb = (volatile uint8_t *)addr + offset;
    uint64_t bsize = ((uint64_t)width * height * 4 + 4095) & ~4095ull;
    if (rx_vmo_create(bsize, 0, &dsp_back_vmo) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE | RX_VM_MAP_RANGE, 0, dsp_back_vmo, 0, bsize, &addr) !=
            RX_OK) {
        fprintf(stderr, "display: no memory for the back buffer\n");
        return 3;
    }
    dsp_back = (uint32_t *)addr;
    static const dsp_backend_t gop = { gop_present, gop_show, gop_snapshot, 0, 0, 0, 0, 0 };
    return dsp_run("display", &gop);
}
