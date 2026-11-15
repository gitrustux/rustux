// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// Window clients (<rustux/window.h>, DESKTOP.md step F4)

#include <errno.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/window.h>
#include <string.h>

#define QSIZE ((int)(sizeof(((rx_window_t *)0)->queue) / sizeof(rx_window_event_t)))

static int send_req(rx_window_t *win, const rx_window_req_t *r, const void *payload, uint32_t len, rx_handle_t h) {
    char buf[sizeof(rx_window_req_t) + RX_WINDOW_MAX_CLIPBOARD];
    if (len > RX_WINDOW_MAX_CLIPBOARD) return -EINVAL;
    memcpy(buf, r, sizeof *r);
    if (len) memcpy(buf + sizeof *r, payload, len);
    rx_status_t s = rx_channel_write(win->ch, 0, buf, (uint32_t)sizeof *r + len, h ? &h : 0, h ? 1 : 0);
    return s == RX_OK ? 0 : -EPIPE;
}

// One message from the compositor: 1, 0 if none by the deadline, or -EPIPE;
// extra (a reply's text) is copied to extra_buf
static int read_one(rx_window_t *win, rx_window_event_t *ev, uint64_t deadline, char *extra_buf, size_t extra_size,
                    uint32_t *extra_len) {
    char buf[sizeof(rx_window_event_t) + RX_WINDOW_MAX_CLIPBOARD];
    for (;;) {
        uint32_t nb = 0, nh = 0;
        rx_handle_t h;
        rx_status_t s = rx_channel_read(win->ch, 0, buf, &h, sizeof buf, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) {
            if (!deadline) return 0;
            uint32_t pend = 0;
            rx_object_wait_one(win->ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, deadline, &pend);
            if (!(pend & (RX_CHANNEL_READABLE | RX_PEER_CLOSED))) return 0;
            continue;
        }
        if (s != RX_OK) return -EPIPE;
        if (nh) rx_handle_close(h);
        if (nb < sizeof *ev) continue;
        memcpy(ev, buf, sizeof *ev);
        if (extra_len) {
            uint32_t n = nb - (uint32_t)sizeof *ev;
            if (extra_buf && extra_size) {
                size_t c = n < extra_size - 1 ? n : extra_size - 1;
                memcpy(extra_buf, buf + sizeof *ev, c);
                extra_buf[c] = 0;
            }
            *extra_len = n;
        }
        if (ev->op == RX_WINDOW_EV_FOCUS) win->focused = (int)ev->flags;
        if (ev->op == RX_WINDOW_EV_FRAME) win->frame_pending = 0;
        return 1;
    }
}

static void enqueue(rx_window_t *win, const rx_window_event_t *ev) {
    if (ev->op == RX_WINDOW_EV_FRAME) return; // nobody waits for it in the queue
    if (win->qlen == QSIZE) {   // full: drop the oldest
        win->qhead = (win->qhead + 1) % QSIZE;
        win->qlen--;
    }
    win->queue[(win->qhead + win->qlen) % QSIZE] = *ev;
    win->qlen++;
}

static int map_buffer(rx_window_t *win, uint32_t w, uint32_t h, rx_handle_t *dup) {
    uint64_t size = ((uint64_t)w * h * 4 + 4095) & ~4095ull, addr = 0;
    rx_handle_t vmo;
    if (!w || !h || w > 16384 || h > 16384) return -EINVAL;
    if (rx_vmo_create(size, 0, &vmo) != RX_OK) return -ENOMEM;
    if (rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ | RX_VM_PERM_WRITE, 0, vmo, 0, size, &addr) != RX_OK ||
        rx_handle_duplicate(vmo, RX_RIGHT_READ | RX_RIGHT_MAP | RX_RIGHT_TRANSFER | RX_RIGHT_DUPLICATE, dup) != RX_OK) {
        rx_handle_close(vmo);
        return -ENOMEM;
    }
    if (win->pixels) rx_vmar_unmap(rx_vmar_root, (uint64_t)win->pixels, ((uint64_t)win->width * win->height * 4 + 4095) & ~4095ull);
    if (win->vmo) rx_handle_close(win->vmo);
    win->vmo = vmo;
    win->pixels = (uint32_t *)addr;
    win->width = w;
    win->height = h;
    return 0;
}

int rx_window_create(rx_window_t *win, int x, int y, uint32_t w, uint32_t h, const char *title) {
    memset(win, 0, sizeof *win);
    int r = rx_service_connect("/svc/window", &win->ch);
    if (r < 0) return r;
    rx_handle_t dup;
    if ((r = map_buffer(win, w, h, &dup)) < 0) {
        rx_handle_close(win->ch);
        return r;
    }
    size_t tl = title ? strlen(title) : 0;
    if (tl > RX_WINDOW_MAX_TITLE) tl = RX_WINDOW_MAX_TITLE;
    rx_window_req_t req = { RX_WINDOW_CREATE, (uint32_t)tl, x, y, w, h, 0 };
    if ((r = send_req(win, &req, title, (uint32_t)tl, dup)) < 0) {
        rx_window_destroy(win);
        return r;
    }
    for (;;) {
        rx_window_event_t ev;
        r = read_one(win, &ev, RX_TIME_INFINITE, 0, 0, 0);
        if (r <= 0) {
            rx_window_destroy(win);
            return -EPIPE;
        }
        if (ev.op == (RX_WINDOW_CREATE | RX_WINDOW_REPLY)) {
            if (ev.status < 0) {
                rx_window_destroy(win);
                return ev.status;
            }
            return 0;
        }
        enqueue(win, &ev);
    }
}

int rx_window_commit(rx_window_t *win, const rx_window_rect_t *rects, int n) {
    if (n < 0 || n > RX_WINDOW_MAX_RECTS) return -EINVAL;
    rx_window_req_t req = { RX_WINDOW_COMMIT, (uint32_t)n, 0, 0, 0, 0, 0 };
    win->frame_pending = 1;
    return send_req(win, &req, rects, (uint32_t)(n * sizeof *rects), 0);
}

int rx_window_wait_frame(rx_window_t *win, uint64_t deadline) {
    while (win->frame_pending) {
        rx_window_event_t ev;
        int r = read_one(win, &ev, deadline, 0, 0, 0);
        if (r <= 0) return r;
        if (ev.op != RX_WINDOW_EV_FRAME) enqueue(win, &ev);
    }
    return 1;
}

int rx_window_next_event(rx_window_t *win, rx_window_event_t *ev, uint64_t deadline) {
    if (win->qlen) {
        *ev = win->queue[win->qhead];
        win->qhead = (win->qhead + 1) % QSIZE;
        win->qlen--;
        return 1;
    }
    return read_one(win, ev, deadline, 0, 0, 0);
}

int rx_window_resize(rx_window_t *win, uint32_t w, uint32_t h) {
    rx_handle_t dup;
    int r = map_buffer(win, w, h, &dup);
    if (r < 0) return r;
    rx_window_req_t req = { RX_WINDOW_BUFFER, 0, 0, 0, w, h, 0 };
    return send_req(win, &req, 0, 0, dup);
}

int rx_window_set_title(rx_window_t *win, const char *title) {
    size_t tl = strlen(title);
    if (tl > RX_WINDOW_MAX_TITLE) tl = RX_WINDOW_MAX_TITLE;
    rx_window_req_t req = { RX_WINDOW_SET_TITLE, (uint32_t)tl, 0, 0, 0, 0, 0 };
    return send_req(win, &req, title, (uint32_t)tl, 0);
}

int rx_window_set_fullscreen(rx_window_t *win, int on) {
    rx_window_req_t req = { RX_WINDOW_SET_FULLSCREEN, 0, 0, 0, 0, 0, on ? 1u : 0u };
    return send_req(win, &req, 0, 0, 0);
}

int rx_window_clipboard_set(rx_window_t *win, const char *text) {
    size_t n = strlen(text);
    if (n > RX_WINDOW_MAX_CLIPBOARD) return -E2BIG;
    rx_window_req_t req = { RX_WINDOW_CLIPBOARD_SET, (uint32_t)n, 0, 0, 0, 0, 0 };
    return send_req(win, &req, text, (uint32_t)n, 0);
}

int rx_window_clipboard_get(rx_window_t *win, char *buf, size_t size) {
    rx_window_req_t req = { RX_WINDOW_CLIPBOARD_GET, 0, 0, 0, 0, 0, 0 };
    int r = send_req(win, &req, 0, 0, 0);
    if (r < 0) return r;
    for (;;) {
        rx_window_event_t ev;
        uint32_t n = 0;
        r = read_one(win, &ev, RX_TIME_INFINITE, buf, size, &n);
        if (r <= 0) return -EPIPE;
        if (ev.op == (RX_WINDOW_CLIPBOARD_GET | RX_WINDOW_REPLY)) return (int)n;
        enqueue(win, &ev);
    }
}

void rx_window_destroy(rx_window_t *win) {
    if (win->pixels) rx_vmar_unmap(rx_vmar_root, (uint64_t)win->pixels, ((uint64_t)win->width * win->height * 4 + 4095) & ~4095ull);
    if (win->vmo) rx_handle_close(win->vmo);
    if (win->ch) rx_handle_close(win->ch); // the compositor removes the window
    memset(win, 0, sizeof *win);
}

int rx_window_set_role(rx_window_t *win, uint32_t role) {
    rx_window_req_t req = { RX_WINDOW_SET_ROLE, 0, 0, 0, 0, 0, role };
    return send_req(win, &req, 0, 0, 0);
}

int rx_window_activate(rx_window_t *win, int id) {
    rx_window_req_t req = { RX_WINDOW_ACTIVATE, 0, id, 0, 0, 0, 0 };
    return send_req(win, &req, 0, 0, 0);
}

int rx_window_lock(rx_window_t *win) {
    rx_window_req_t req = { RX_WINDOW_LOCK, 0, 0, 0, 0, 0, 0 };
    return send_req(win, &req, 0, 0, 0);
}

int rx_window_notify(rx_window_t *win, const char *text) {
    size_t n = strlen(text);
    if (n > 120) n = 120;
    rx_window_req_t req = { RX_WINDOW_NOTIFY, (uint32_t)n, 0, 0, 0, 0, 0 };
    if (win) return send_req(win, &req, text, (uint32_t)n, 0);
    rx_window_t tmp;
    memset(&tmp, 0, sizeof tmp);
    int r = rx_service_connect("/svc/window", &tmp.ch);
    if (r < 0) return r;
    r = send_req(&tmp, &req, text, (uint32_t)n, 0);
    rx_handle_close(tmp.ch);
    return r;
}
