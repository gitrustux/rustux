// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The display service's server side (<rustux/display.h>, docs/DISPLAY.md),
// shared by its two programs (DESKTOP.md step F8): display (the UEFI GOP
// framebuffer) and virtio-gpu (QEMU's, KVM's and Proxmox's virtual GPU).
// The program supplies a backend: it allocates the back buffer, puts
// presented rectangles on the screen, switches between the owner's
// graphics and the text console, and may draw the pointer itself (a
// hardware cursor) and change size at run time. This file publishes
// /svc/display and answers the clients.

#ifndef DISPLAYSVC_H
#define DISPLAYSVC_H

#include <errno.h>
#include <rustux/display.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DSP_MAX_CLIENTS 16

typedef struct {
    // Show these rectangles of the back buffer (n 0: all of it); called
    // only while the graphics are shown
    void (*present)(const rx_display_rect_t *r, int n);
    // 1: the owner's graphics on the screen; 0: the text console
    void (*show)(int graphics);
    // What is on the screen now: a VMO of width x height 0x00RRGGBB, or 0
    rx_handle_t (*snapshot)(uint32_t *w, uint32_t *h);
    // A hardware cursor (0 if there is none): its image, 64 x 64
    // 0xAARRGGBB, with the hot spot; then moves (x, y on the screen)
    int (*cursor_set)(const uint32_t *argb, uint32_t hot_x, uint32_t hot_y);
    void (*cursor_move)(int x, int y, int visible);
    // Something more to wait for (an interrupt, a timer): fills items,
    // returns how many (at most 4); handle() is told what was pending
    int (*wait_items)(rx_wait_item_t *items);
    void (*handle)(const rx_wait_item_t *items, int n);
    uint32_t caps;                  // RX_DISPLAY_CAP_*
} dsp_backend_t;

static const dsp_backend_t *dsp;
static uint32_t dsp_width, dsp_height;
static uint32_t dsp_primary_w, dsp_primary_h; // the first head's part (0: all)
static rx_handle_t dsp_back_vmo;   // the back buffer the owner draws into
static uint32_t *dsp_back;         // mapped here
static rx_handle_t dsp_clients[DSP_MAX_CLIENTS];
static int dsp_nclients;
static rx_handle_t dsp_owner;      // 0: nobody draws
static int dsp_graphics;           // the owner's pixels are on the screen
static rx_handle_t dsp_listen;

static void dsp_reply(rx_handle_t ch, uint32_t op, int32_t status, uint32_t a, uint32_t b, rx_handle_t h) {
    rx_display_msg_t m = { op | RX_DISPLAY_REPLY, status, a, b };
    rx_channel_write(ch, 0, &m, sizeof m, h ? &h : 0, h ? 1 : 0);
}

static void dsp_event(uint32_t op, uint32_t a, uint32_t b, rx_handle_t h) {
    if (!dsp_owner) {
        if (h) rx_handle_close(h);
        return;
    }
    rx_display_msg_t m = { op, 0, a, b };
    if (rx_channel_write(dsp_owner, 0, &m, sizeof m, h ? &h : 0, h ? 1 : 0) != RX_OK && h) rx_handle_close(h);
}

// Put the owner's graphics on the screen, or give it to the console
static void dsp_show(int on) {
    if (on == dsp_graphics) return;
    dsp_graphics = on;
    dsp->show(on);
    if (on) dsp->present(0, 0);
    dsp_event(RX_DISPLAY_EV_VISIBLE, (uint32_t)on, 0, 0);
}

static void dsp_release(void) {
    dsp_show(0);
    dsp_owner = 0;
    if (dsp->cursor_move) dsp->cursor_move(0, 0, 0);
}

// The backend changed the size (and made a new back buffer): the owner
// gets the new one with RX_DISPLAY_EV_RESIZE
__attribute__((unused)) static void dsp_resized(void) {
    if (!dsp_owner) return;
    rx_handle_t dup;
    if (rx_handle_duplicate(dsp_back_vmo, RX_RIGHT_READ | RX_RIGHT_WRITE | RX_RIGHT_MAP | RX_RIGHT_TRANSFER, &dup) != RX_OK) return;
    dsp_event(RX_DISPLAY_EV_RESIZE, dsp_width, dsp_height, dup);
}

static void dsp_drop_client(int k) {
    if (dsp_clients[k] == dsp_owner) dsp_release();
    rx_handle_close(dsp_clients[k]);
    dsp_clients[k] = dsp_clients[--dsp_nclients];
}

// One request: 1 handled, 2 none waiting, 0 the client is gone
static int dsp_client_message(int k) {
    rx_handle_t ch = dsp_clients[k];
    char buf[sizeof(rx_display_msg_t) + RX_DISPLAY_MAX_RECTS * sizeof(rx_display_rect_t)];
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_sender_t who;
    rx_status_t s = rx_channel_read_creds(ch, buf, &h, sizeof buf, 1, &nb, &nh, &who);
    if (s == RX_ERR_SHOULD_WAIT) return 2;
    if (s != RX_OK) return 0;
    if (nb < sizeof(rx_display_msg_t)) {
        if (nh) rx_handle_close(h);
        return 1;
    }
    rx_display_msg_t m;
    memcpy(&m, buf, sizeof m);
    int root = who.euid == 0;
    if (nh && m.op != RX_DISPLAY_OP_CURSOR_SET) {
        rx_handle_close(h);
        nh = 0;
    }
    switch (m.op) {
    case RX_DISPLAY_OP_INFO:
        dsp_reply(ch, m.op, 0, dsp_width, dsp_height, 0);
        break;
    case RX_DISPLAY_OP_PRIMARY:
        dsp_reply(ch, m.op, 0, dsp_primary_w ? dsp_primary_w : dsp_width, dsp_primary_h ? dsp_primary_h : dsp_height, 0);
        break;
    case RX_DISPLAY_OP_CAPS:
        dsp_reply(ch, m.op, 0, dsp->caps, 0, 0);
        break;
    case RX_DISPLAY_OP_ACQUIRE: {
        if (!root) {
            dsp_reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        if (dsp_owner && dsp_owner != ch) {
            dsp_reply(ch, m.op, -EBUSY, 0, 0, 0);
            break;
        }
        rx_handle_t dup;
        if (rx_handle_duplicate(dsp_back_vmo, RX_RIGHT_READ | RX_RIGHT_WRITE | RX_RIGHT_MAP | RX_RIGHT_TRANSFER, &dup) != RX_OK) {
            dsp_reply(ch, m.op, -EIO, 0, 0, 0);
            break;
        }
        // A new owner starts from black
        memset(dsp_back, 0, (size_t)dsp_width * dsp_height * 4);
        dsp_owner = ch;
        dsp_reply(ch, m.op, 0, dsp_width, dsp_height, dup);
        if (dsp_graphics) dsp->present(0, 0);
        else dsp_show(1);
        break;
    }
    case RX_DISPLAY_OP_RELEASE:
        if (ch == dsp_owner) dsp_release();
        dsp_reply(ch, m.op, 0, 0, 0, 0);
        break;
    case RX_DISPLAY_OP_PRESENT: {
        if (ch != dsp_owner) {
            dsp_reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        uint32_t n = m.a;
        if (n > RX_DISPLAY_MAX_RECTS || nb < sizeof m + n * sizeof(rx_display_rect_t)) {
            dsp_reply(ch, m.op, -EINVAL, 0, 0, 0);
            break;
        }
        // Rectangles of an older size (a resize crossed the frame) are cut
        rx_display_rect_t rs[RX_DISPLAY_MAX_RECTS];
        int k2 = 0;
        for (uint32_t i = 0; i < n; i++) {
            rx_display_rect_t r;
            memcpy(&r, buf + sizeof m + i * sizeof r, sizeof r);
            if (r.x >= dsp_width || r.y >= dsp_height || !r.w || !r.h) continue;
            if (r.w > dsp_width - r.x) r.w = dsp_width - r.x;
            if (r.h > dsp_height - r.y) r.h = dsp_height - r.y;
            rs[k2++] = r;
        }
        if (dsp_graphics && (k2 || !n)) dsp->present(n ? rs : 0, n ? k2 : 0);
        dsp_reply(ch, m.op, 0, (uint32_t)dsp_graphics, 0, 0);
        break;
    }
    case RX_DISPLAY_OP_SNAPSHOT: {
        if (!root) {
            dsp_reply(ch, m.op, -EPERM, 0, 0, 0);
            break;
        }
        uint32_t w = 0, hh = 0;
        rx_handle_t v = dsp->snapshot(&w, &hh);
        dsp_reply(ch, m.op, v ? 0 : -ENOMEM, w, hh, v);
        break;
    }
    case RX_DISPLAY_OP_SWITCH:
        // No reply: init sends it and goes on
        if (root) dsp_show(m.a == RX_DISPLAY_GRAPHICS && dsp_owner != 0);
        break;
    case RX_DISPLAY_OP_KEY:
        if (root && dsp_graphics) dsp_event(RX_DISPLAY_EV_KEY, m.a, 0, 0);
        break;
    case RX_DISPLAY_OP_POINTER:
        if (root && dsp_graphics && dsp_owner) dsp_event(RX_DISPLAY_EV_POINTER, m.a, m.b, 0);
        break;
    case RX_DISPLAY_OP_CURSOR_SET: {
        // The owner's pointer image: a VMO of 64 x 64 0xAARRGGBB
        int st = -ENOTSUP;
        if (ch != dsp_owner) st = -EPERM;
        else if (dsp->cursor_set && nh) {
            uint32_t *img = malloc(64 * 64 * 4);
            st = -ENOMEM;
            if (img && rx_vmo_read(h, img, 0, 64 * 64 * 4) == RX_OK) st = dsp->cursor_set(img, m.a & 0xFFFF, m.a >> 16);
            free(img);
        }
        if (nh) rx_handle_close(h);
        dsp_reply(ch, m.op, st, 0, 0, 0);
        break;
    }
    case RX_DISPLAY_OP_CURSOR_MOVE:
        // No reply (it comes with every movement)
        if (ch == dsp_owner && dsp->cursor_move && dsp_graphics) dsp->cursor_move((int)(m.a & 0xFFFF), (int)(m.a >> 16), (int)m.b);
        break;
    default:
        dsp_reply(ch, m.op, -ENOSYS, 0, 0, 0);
    }
    return 1;
}

static void dsp_accept(void) {
    char buf[64];
    rx_handle_t h;
    uint32_t nb = 0, nh = 0;
    while (rx_channel_read(dsp_listen, 0, buf, &h, sizeof buf, 1, &nb, &nh) == RX_OK) {
        if (!nh) continue;
        if (dsp_nclients == DSP_MAX_CLIENTS) rx_handle_close(h);
        else dsp_clients[dsp_nclients++] = h;
        nh = 0;
    }
}

// Publish /svc/display and serve it for ever (prog: the program's name)
static int dsp_run(const char *prog, const dsp_backend_t *backend) {
    dsp = backend;
    rx_handle_t theirs;
    if (rx_channel_create(0, &dsp_listen, &theirs) != RX_OK || rx_service_publish("/svc/display", theirs) < 0) {
        fprintf(stderr, "%s: cannot publish /svc/display\n", prog);
        return 3;
    }
    for (;;) {
        rx_wait_item_t items[1 + DSP_MAX_CLIENTS + 4];
        int ni = 0;
        items[ni++] = (rx_wait_item_t){ dsp_listen, RX_CHANNEL_READABLE, 0 };
        for (int k = 0; k < dsp_nclients; k++) items[ni++] = (rx_wait_item_t){ dsp_clients[k], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        int nb = dsp->wait_items ? dsp->wait_items(items + ni) : 0;
        int base = ni;
        ni += nb;
        rx_object_wait_many(items, (uint64_t)ni, RX_TIME_INFINITE);
        if (nb) dsp->handle(items + base, nb);
        int nc = dsp_nclients;
        for (int k = nc - 1; k >= 0; k--) {
            if (!items[1 + k].pending) continue;
            // Every message waiting (a closed channel reads its last
            // messages, then fails)
            int r = 1;
            for (int i = 0; i < 64 && r == 1; i++) r = dsp_client_message(k);
            if (!r) dsp_drop_client(k);
        }
        if (items[0].pending & RX_CHANNEL_READABLE) dsp_accept();
    }
}

#endif
