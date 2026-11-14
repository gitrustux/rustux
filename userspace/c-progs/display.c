// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// display: the display service (DESKTOP.md step F2, docs/DISPLAY.md).
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

#include <errno.h>
#include <rustux/display.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_CLIENTS 16

static uint32_t width, height, stride, offset;
static uint32_t rshift, gshift, bshift;
static int identity;            // the framebuffer is 0x00RRGGBB already
static volatile uint8_t *fb;    // the framebuffer (its first pixel)
static rx_handle_t claim;

static rx_handle_t back_vmo;
static uint32_t *back;          // the back buffer, width * 4 bytes a row

static rx_handle_t clients[MAX_CLIENTS];
static int nclients;
static rx_handle_t owner;       // 0: nobody draws
static int graphics;            // the owner's pixels are on the screen

static uint32_t mask_shift(uint32_t m) {
    uint32_t s = 0;
    while (s < 32 && !((m >> s) & 1)) s++;
    return s;
}

// Copy a rectangle of the back buffer to the screen
static void copy_rect(uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    if (x >= width || y >= height) return;
    if (w > width - x) w = width - x;
    if (h > height - y) h = height - y;
    for (uint32_t r = y; r < y + h; r++) {
        const uint32_t *src = back + (uint64_t)r * width + x;
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

static void reply(rx_handle_t ch, uint32_t op, int32_t status, uint32_t a, uint32_t b, rx_handle_t h) {
    rx_display_msg_t m = { op | RX_DISPLAY_REPLY, status, a, b };
    rx_channel_write(ch, 0, &m, sizeof m, h ? &h : 0, h ? 1 : 0);
}

static void event(uint32_t op, uint32_t a) {
    if (!owner) return;
    rx_display_msg_t m = { op, 0, a, 0 };
    rx_channel_write(owner, 0, &m, sizeof m, 0, 0);
}

// Put the owner's graphics on the screen, or give it to the console
static void show_graphics(int on) {
    if (on == graphics) return;
    if (on) {
        rx_display_console(claim, RX_DISPLAY_GRAPHICS);
        graphics = 1;
        copy_rect(0, 0, width, height);
    } else {
        graphics = 0;
        rx_display_console(claim, RX_DISPLAY_TEXT);
    }
    event(RX_DISPLAY_EV_VISIBLE, (uint32_t)on);
}

static void release(void) {
    show_graphics(0);
    owner = 0;
}

// A snapshot of what is on the screen, from the framebuffer itself
static rx_handle_t snapshot(void) {
    rx_handle_t vmo;
    uint64_t size = (uint64_t)width * height * 4;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) return 0;
    uint32_t *row = malloc((size_t)width * 4);
    if (!row) {
        rx_handle_close(vmo);
        return 0;
    }
    for (uint32_t r = 0; r < height; r++) {
        volatile const uint32_t *src = (volatile const uint32_t *)(fb + (uint64_t)r * stride);
        for (uint32_t i = 0; i < width; i++) {
            uint32_t p = src[i];
            row[i] = identity ? p & 0xFFFFFF
                              : ((p >> rshift) & 0xFF) << 16 | ((p >> gshift) & 0xFF) << 8 | ((p >> bshift) & 0xFF);
        }
        rx_vmo_write(vmo, row, (uint64_t)r * width * 4, (uint64_t)width * 4);
    }
    free(row);
    return vmo;
}

static void drop_client(int k) {
    if (clients[k] == owner) release();
    rx_handle_close(clients[k]);
    clients[k] = clients[--nclients];
}

// One request: 1 handled, 2 none waiting, 0 the client is gone
static int client_message(int k) {
    rx_handle_t ch = clients[k];
    char buf[sizeof(rx_display_msg_t) + RX_DISPLAY_MAX_RECTS * sizeof(rx_display_rect_t)];
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_sender_t who;
    rx_status_t s = rx_channel_read_creds(ch, buf, &h, sizeof buf, 1, &nb, &nh, &who);
    if (s == RX_ERR_SHOULD_WAIT) return 2;
    if (s != RX_OK) return 0;
    if (nh) rx_handle_close(h);
    if (nb < sizeof(rx_display_msg_t)) return 1;
    rx_display_msg_t m;
    memcpy(&m, buf, sizeof m);
    int root = who.euid == 0;
    switch (m.op) {
    case RX_DISPLAY_OP_INFO:
        reply(ch, m.op, 0, width, height, 0);
        break;
    case RX_DISPLAY_OP_ACQUIRE: {
        if (!root) {
            reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        if (owner && owner != ch) {
            reply(ch, m.op, -EBUSY, 0, 0, 0);
            break;
        }
        rx_handle_t dup;
        if (rx_handle_duplicate(back_vmo, RX_RIGHT_READ | RX_RIGHT_WRITE | RX_RIGHT_MAP | RX_RIGHT_TRANSFER, &dup) != RX_OK) {
            reply(ch, m.op, -EIO, 0, 0, 0);
            break;
        }
        // A new owner starts from black
        memset(back, 0, (size_t)width * height * 4);
        owner = ch;
        reply(ch, m.op, 0, width, height, dup);
        if (graphics) copy_rect(0, 0, width, height);
        else show_graphics(1);
        break;
    }
    case RX_DISPLAY_OP_RELEASE:
        if (ch == owner) release();
        reply(ch, m.op, 0, 0, 0, 0);
        break;
    case RX_DISPLAY_OP_PRESENT: {
        if (ch != owner) {
            reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        uint32_t n = m.a;
        if (n > RX_DISPLAY_MAX_RECTS || nb < sizeof m + n * sizeof(rx_display_rect_t)) {
            reply(ch, m.op, -EINVAL, 0, 0, 0);
            break;
        }
        if (graphics) {
            if (!n) copy_rect(0, 0, width, height);
            for (uint32_t i = 0; i < n; i++) {
                rx_display_rect_t r;
                memcpy(&r, buf + sizeof m + i * sizeof r, sizeof r);
                copy_rect(r.x, r.y, r.w, r.h);
            }
        }
        reply(ch, m.op, 0, (uint32_t)graphics, 0, 0);
        break;
    }
    case RX_DISPLAY_OP_SNAPSHOT: {
        if (!root) {
            reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        rx_handle_t v = snapshot();
        reply(ch, m.op, v ? 0 : -ENOMEM, width, height, v);
        break;
    }
    case RX_DISPLAY_OP_SWITCH:
        // No reply: init sends it and goes on
        if (root) show_graphics(m.a == RX_DISPLAY_GRAPHICS && owner != 0);
        break;
    case RX_DISPLAY_OP_KEY:
        if (root && graphics) event(RX_DISPLAY_EV_KEY, m.a);
        break;
    default:
        reply(ch, m.op, -ENOSYS, 0, 0, 0);
    }
    return 1;
}

static void accept_clients(rx_handle_t listen) {
    char buf[64];
    rx_handle_t h;
    uint32_t nb = 0, nh = 0;
    while (rx_channel_read(listen, 0, buf, &h, sizeof buf, 1, &nb, &nh) == RX_OK) {
        if (!nh) continue;
        if (nclients == MAX_CLIENTS) rx_handle_close(h);
        else clients[nclients++] = h;
    }
}

int main(int argc, char **argv) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    rx_handle_t fb_vmo = rx_take_startup_handle(RX_PA_USER0);
    claim = rx_take_startup_handle(RX_PA_USER1);
    if (argc != 8 || !fb_vmo || !claim) {
        fprintf(stderr, "display: start me from devmgr\n");
        return 2;
    }
    width = (uint32_t)strtoul(argv[1], 0, 10);
    height = (uint32_t)strtoul(argv[2], 0, 10);
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
    if (rx_vmo_create(bsize, 0, &back_vmo) != RX_OK ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE | RX_VM_MAP_RANGE, 0, back_vmo, 0, bsize, &addr) !=
            RX_OK) {
        fprintf(stderr, "display: no memory for the back buffer\n");
        return 3;
    }
    back = (uint32_t *)addr;
    rx_handle_t listen, theirs;
    if (rx_channel_create(0, &listen, &theirs) != RX_OK || rx_service_publish("/svc/display", theirs) < 0) {
        fprintf(stderr, "display: cannot publish /svc/display\n");
        return 3;
    }
    for (;;) {
        rx_wait_item_t items[1 + MAX_CLIENTS];
        int ni = 0;
        items[ni++] = (rx_wait_item_t){ listen, RX_CHANNEL_READABLE, 0 };
        for (int k = 0; k < nclients; k++) items[ni++] = (rx_wait_item_t){ clients[k], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        rx_object_wait_many(items, (uint64_t)ni, RX_TIME_INFINITE);
        for (int k = nclients - 1; k >= 0; k--) {
            if (!items[1 + k].pending) continue;
            // Every message waiting (a closed channel reads its last
            // messages, then fails)
            int r = 1;
            for (int i = 0; i < 64 && r == 1; i++) r = client_message(k);
            if (!r) drop_client(k);
        }
        if (items[0].pending & RX_CHANNEL_READABLE) accept_clients(listen);
    }
}
