// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Clients of the display service (<rustux/display.h>, DESKTOP.md step F2)

#include <errno.h>
#include <rustux/display.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdlib.h>
#include <string.h>

// A new back buffer (EV_RESIZE): map it in place of the old one
static void resized(rx_display_t *d, const rx_display_msg_t *m, rx_handle_t vmo) {
    uint64_t addr = 0, size = ((uint64_t)m->a * m->b * 4 + 4095) & ~4095ull;
    if (!vmo || rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK) {
        if (vmo) rx_handle_close(vmo);
        return;
    }
    if (d->pixels) rx_vmar_unmap(rx_vmar_root, (uint64_t)d->pixels, ((uint64_t)d->width * d->height * 4 + 4095) & ~4095ull);
    if (d->vmo) rx_handle_close(d->vmo);
    d->vmo = vmo;
    d->pixels = (uint32_t *)addr;
    d->width = m->a;
    d->height = m->b;
}

// Wait for the reply to `op`; events met on the way go to d's queue (when d)
static int wait_reply(rx_handle_t ch, rx_display_t *d, uint32_t op, rx_display_msg_t *reply, rx_handle_t *h) {
    for (;;) {
        rx_display_msg_t m;
        rx_handle_t got = 0;
        uint32_t nb = 0, nh = 0;
        rx_status_t s = rx_channel_read(ch, 0, &m, &got, sizeof m, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) {
            uint32_t pend = 0;
            rx_object_wait_one(ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
            continue;
        }
        if (s != RX_OK) return -EPIPE;
        if (nb < sizeof m) {
            if (nh) rx_handle_close(got);
            continue;
        }
        if (m.op == (op | RX_DISPLAY_REPLY)) {
            *reply = m;
            if (h) *h = nh ? got : 0;
            else if (nh) rx_handle_close(got);
            return 0;
        }
        if (m.op == RX_DISPLAY_EV_RESIZE && d) resized(d, &m, nh ? got : 0);
        else if (nh) rx_handle_close(got);
        if ((m.op & RX_DISPLAY_EVENT) && d) {
            if (m.op == RX_DISPLAY_EV_VISIBLE) d->visible = (int)m.a;
            if (d->qlen < (int)(sizeof d->queue / sizeof d->queue[0])) {
                d->queue[(d->qhead + d->qlen) % 32] = m;
                d->qlen++;
            }
        }
    }
}

static int call(rx_handle_t ch, rx_display_t *d, uint32_t op, uint32_t a, const void *extra, uint32_t elen,
                rx_display_msg_t *reply, rx_handle_t *h) {
    char buf[sizeof(rx_display_msg_t) + RX_DISPLAY_MAX_RECTS * sizeof(rx_display_rect_t)];
    rx_display_msg_t m = { op, 0, a, 0 };
    memcpy(buf, &m, sizeof m);
    if (elen) memcpy(buf + sizeof m, extra, elen);
    if (rx_channel_write(ch, 0, buf, (uint32_t)sizeof m + elen, 0, 0) != RX_OK) return -EPIPE;
    int r = wait_reply(ch, d, op, reply, h);
    if (r < 0) return r;
    return reply->status;
}

int rx_display_open(rx_display_t *d) {
    memset(d, 0, sizeof *d);
    int r = rx_service_connect("/svc/display", &d->ch);
    if (r < 0) return r;
    rx_display_msg_t rep;
    rx_handle_t vmo = 0;
    r = call(d->ch, d, RX_DISPLAY_OP_ACQUIRE, 0, 0, 0, &rep, &vmo);
    if (r < 0 || !vmo) {
        if (vmo) rx_handle_close(vmo);
        rx_handle_close(d->ch);
        return r < 0 ? r : -EIO;
    }
    d->width = rep.a;
    d->height = rep.b;
    uint64_t addr = 0, size = (uint64_t)d->width * d->height * 4;
    if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, (size + 4095) & ~4095ull, &addr) != RX_OK) {
        rx_handle_close(vmo);
        rx_handle_close(d->ch);
        return -ENOMEM;
    }
    d->vmo = vmo;
    d->pixels = (uint32_t *)addr;
    d->visible = 1;
    if (call(d->ch, d, RX_DISPLAY_OP_CAPS, 0, 0, 0, &rep, 0) == 0) d->caps = rep.a;
    return 0;
}

int rx_display_cursor_set(rx_display_t *d, const uint32_t *argb, uint32_t hot_x, uint32_t hot_y) {
    rx_handle_t vmo;
    if (rx_vmo_create(64 * 64 * 4, 0, &vmo) != RX_OK) return -ENOMEM;
    if (rx_vmo_write(vmo, argb, 0, 64 * 64 * 4) != RX_OK) {
        rx_handle_close(vmo);
        return -EIO;
    }
    rx_display_msg_t m = { RX_DISPLAY_OP_CURSOR_SET, 0, hot_x | hot_y << 16, 0 };
    if (rx_channel_write(d->ch, 0, &m, sizeof m, &vmo, 1) != RX_OK) {
        rx_handle_close(vmo);
        return -EPIPE;
    }
    rx_display_msg_t rep;
    int r = wait_reply(d->ch, d, RX_DISPLAY_OP_CURSOR_SET, &rep, 0);
    return r < 0 ? r : rep.status;
}

int rx_display_primary(rx_display_t *d, uint32_t *w, uint32_t *h) {
    rx_display_msg_t rep;
    if (call(d->ch, d, RX_DISPLAY_OP_PRIMARY, 0, 0, 0, &rep, 0) < 0) {
        *w = d->width;
        *h = d->height;
        return 0;
    }
    *w = rep.a;
    *h = rep.b;
    return 0;
}

int rx_display_cursor_move(rx_display_t *d, int x, int y, int visible) {
    rx_display_msg_t m = { RX_DISPLAY_OP_CURSOR_MOVE, 0, (uint32_t)(x & 0xFFFF) | (uint32_t)(y & 0xFFFF) << 16, visible ? 1u : 0u };
    return rx_channel_write(d->ch, 0, &m, sizeof m, 0, 0) == RX_OK ? 0 : -EPIPE;
}

int rx_display_present(rx_display_t *d, const rx_display_rect_t *rects, int n) {
    if (n < 0 || n > RX_DISPLAY_MAX_RECTS) return -EINVAL;
    rx_display_msg_t rep;
    int r = call(d->ch, d, RX_DISPLAY_OP_PRESENT, (uint32_t)n, rects, (uint32_t)(n * sizeof *rects), &rep, 0);
    if (r < 0) return r;
    d->visible = (int)rep.a;
    return (int)rep.a;
}

int rx_display_poll(rx_display_t *d, rx_display_msg_t *ev) {
    if (d->qlen) {
        *ev = d->queue[d->qhead];
        d->qhead = (d->qhead + 1) % 32;
        d->qlen--;
        return 1;
    }
    for (;;) {
        rx_display_msg_t m;
        rx_handle_t got = 0;
        uint32_t nb = 0, nh = 0;
        rx_status_t s = rx_channel_read(d->ch, 0, &m, &got, sizeof m, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) return 0;
        if (s != RX_OK) return -EPIPE;
        if (nb >= sizeof m && m.op == RX_DISPLAY_EV_RESIZE) resized(d, &m, nh ? got : 0);
        else if (nh) rx_handle_close(got);
        if (nb < sizeof m || !(m.op & RX_DISPLAY_EVENT)) continue;
        if (m.op == RX_DISPLAY_EV_VISIBLE) d->visible = (int)m.a;
        *ev = m;
        return 1;
    }
}

void rx_display_close(rx_display_t *d) {
    if (d->pixels) rx_vmar_unmap(rx_vmar_root, (uint64_t)d->pixels, ((uint64_t)d->width * d->height * 4 + 4095) & ~4095ull);
    if (d->vmo) rx_handle_close(d->vmo);
    if (d->ch) rx_handle_close(d->ch);  // the service shows the console again
    memset(d, 0, sizeof *d);
}

int rx_display_snapshot(uint32_t **pixels, uint32_t *width, uint32_t *height) {
    rx_handle_t ch;
    int r = rx_service_connect("/svc/display", &ch);
    if (r < 0) return r;
    rx_display_msg_t rep;
    rx_handle_t vmo = 0;
    r = call(ch, 0, RX_DISPLAY_OP_SNAPSHOT, 0, 0, 0, &rep, &vmo);
    rx_handle_close(ch);
    if (r < 0 || !vmo) {
        if (vmo) rx_handle_close(vmo);
        return r < 0 ? r : -EIO;
    }
    uint64_t size = (uint64_t)rep.a * rep.b * 4;
    uint32_t *p = malloc(size ? size : 1);
    if (!p) {
        rx_handle_close(vmo);
        return -ENOMEM;
    }
    rx_status_t s = rx_vmo_read(vmo, p, 0, size);
    rx_handle_close(vmo);
    if (s != RX_OK) {
        free(p);
        return -EIO;
    }
    *pixels = p;
    *width = rep.a;
    *height = rep.b;
    return 0;
}
