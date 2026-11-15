// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// compositor: windows on the screen (DESKTOP.md step F4, docs/WINDOWS.md,
// <rustux/window.h>).
//
// Takes the screen from the display service (it becomes its owner) and,
// while graphics are shown, the input from the input service. Clients
// connect to /svc/window, one channel a window, and share a buffer; the
// compositor keeps a copy of what each committed, draws title bars and
// borders around them, the cursor on top, and presents only what changed.
//
// Window management: click to focus and raise; drag the title bar to
// move, the bottom and right edges and the corner to resize; the title
// bar's buttons minimise, maximise (or restore) and close; double-click
// the title bar to maximise; Alt+Tab focuses the next window (and brings
// a minimised one back); Alt+F4 asks the focused window to close; F11
// makes it full screen and back. Keys go to the focused window only;
// the pointer too, with coordinates relative to its content (while a
// button is held, to the window it went down in).
//
// A client whose channel closes (it ended, or was killed) loses its
// window; one that does not close within 5 s of a close request too. If
// the compositor ends, the display service gives the screen back to the
// text console.
//
// For the desktop session (step F6): a window can be the panel (docked at
// the bottom, always on top, never focused; it hears of every window and
// can activate them); clients can show notifications (top right, 5 s);
// and the screen can be locked (the session's user's password unlocks
// it). The desktop program (desktop --run) starts the compositor with a
// control channel on which it names the session's user ("user NAME", or
// "user" for none: no lock then).
//
//   compositor [-v]     -v: print what happens (for the tests)
//
// Root only (the display and input services give the screen and the
// input to root).

#include <errno.h>
#include <rustux/display.h>
#include <rustux/font8x16.h>
#include <rustux/input.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/window.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "userdb.h"

#define MAX_WINDOWS 32
#define TITLE_H 24      // title bar height
#define BORDER 1
#define GRIP 5          // resize zone outside the border
#define BTN 16          // title bar button size
#define MIN_W 64
#define MIN_H 32

// Dracula
#define C_DESKTOP     0x1E1F29
#define C_TITLE       0x44475A
#define C_TITLE_OFF   0x2E303E
#define C_TEXT        0xF8F8F2
#define C_TEXT_OFF    0x8A8FA8
#define C_BORDER      0xBD93F9
#define C_BORDER_OFF  0x44475A
#define C_CLOSE       0xFF5555
#define C_BUTTON      0x6272A4

typedef struct {
    int used;
    rx_handle_t ch;
    char title[RX_WINDOW_MAX_TITLE + 1];
    int x, y;                   // content's top left on the screen
    uint32_t w, h;              // content size (the committed buffer's)
    uint32_t *pix;              // our copy of what was committed
    rx_handle_t vmo;            // the client's buffer
    const uint32_t *client;     // mapped read-only
    uint64_t client_size;
    int minimized, maximized, fullscreen;
    int sx, sy;                 // geometry before maximising or full screen
    uint32_t sw, sh;
    int committed;              // something committed since the last FRAME
    uint64_t close_deadline;    // after a close request, 0 if none
    uint32_t want_w, want_h;    // the size asked for with CONFIGURE (0: none pending)
    int panel;                  // the panel: no frame, docked at the bottom, on top, never focused
} window_t;

static window_t wins[MAX_WINDOWS];
static int order[MAX_WINDOWS], norder;  // z-order, bottom first (indexes into wins)
static int focus = -1;
static int verbose;

static rx_display_t d;
static uint32_t W, H;
static rx_handle_t input_ch;
static int visible = 1;

static char clipboard[RX_WINDOW_MAX_CLIPBOARD + 1];
static size_t clipboard_len;

static int panel_h;             // the screen's bottom rows the panel takes
static char session_user[64];   // who unlocks the screen ("" without a session)
static int locked;
static char lock_pw[65];
static int lock_failed;

#define MAX_TOASTS 3
static struct { char text[121]; uint64_t until; } toasts[MAX_TOASTS];
static int ntoasts;

#define vlog(...) do { if (verbose) { printf("compositor: " __VA_ARGS__); fflush(stdout); } } while (0)

// ---- damage ----

#define MAX_DAMAGE 32
static rx_display_rect_t damage[MAX_DAMAGE];
static int ndamage;

static int clip(int *x, int *y, int *w, int *h) {
    if (*x < 0) { *w += *x; *x = 0; }
    if (*y < 0) { *h += *y; *y = 0; }
    if (*x + *w > (int)W) *w = (int)W - *x;
    if (*y + *h > (int)H) *h = (int)H - *y;
    return *w > 0 && *h > 0;
}

static void add_damage(int x, int y, int w, int h) {
    if (!clip(&x, &y, &w, &h)) return;
    if (ndamage == MAX_DAMAGE) {
        // Too many: merge everything into one rectangle
        int x0 = x, y0 = y, x1 = x + w, y1 = y + h;
        for (int i = 0; i < ndamage; i++) {
            if ((int)damage[i].x < x0) x0 = (int)damage[i].x;
            if ((int)damage[i].y < y0) y0 = (int)damage[i].y;
            if ((int)(damage[i].x + damage[i].w) > x1) x1 = (int)(damage[i].x + damage[i].w);
            if ((int)(damage[i].y + damage[i].h) > y1) y1 = (int)(damage[i].y + damage[i].h);
        }
        ndamage = 1;
        damage[0] = (rx_display_rect_t){ (uint32_t)x0, (uint32_t)y0, (uint32_t)(x1 - x0), (uint32_t)(y1 - y0) };
        return;
    }
    damage[ndamage++] = (rx_display_rect_t){ (uint32_t)x, (uint32_t)y, (uint32_t)w, (uint32_t)h };
}

// The whole frame of a window (border, title bar, content) and the resize grips
static void frame_rect(const window_t *w, int *x, int *y, int *fw, int *fh) {
    if (w->fullscreen || w->panel) {
        *x = w->x; *y = w->y; *fw = (int)w->w; *fh = (int)w->h;
        return;
    }
    *x = w->x - BORDER;
    *y = w->y - TITLE_H - BORDER;
    *fw = (int)w->w + 2 * BORDER;
    *fh = (int)w->h + TITLE_H + 2 * BORDER;
}

static void damage_window(const window_t *w) {
    int x, y, fw, fh;
    frame_rect(w, &x, &y, &fw, &fh);
    add_damage(x, y, fw, fh);
}

// ---- the cursor ----

static int cx, cy;
static uint32_t buttons;
static const char *arrow[] = {
    "1", "11", "121", "1221", "12221", "122221", "1222221", "12222221", "122222221", "1222222221",
    "12222211111", "1221221", "121 1221", "11  1221", "1    1221", "     1221", "      11",
};
#define ARROW_W 11
#define ARROW_H 17

// ---- drawing ----

static uint32_t *fbp;   // the display's back buffer

static void fill(int x, int y, int w, int h, uint32_t c, const rx_display_rect_t *r) {
    // Clip to r
    int x0 = x > (int)r->x ? x : (int)r->x, y0 = y > (int)r->y ? y : (int)r->y;
    int x1 = x + w < (int)(r->x + r->w) ? x + w : (int)(r->x + r->w);
    int y1 = y + h < (int)(r->y + r->h) ? y + h : (int)(r->y + r->h);
    for (int yy = y0; yy < y1; yy++) {
        uint32_t *p = fbp + (uint64_t)yy * W;
        for (int xx = x0; xx < x1; xx++) p[xx] = c;
    }
}

// The next character of UTF-8 text as the 8x16 font has it: ASCII, a few
// look-alikes (dashes, x, bullet, quotes), '?' for the rest
static unsigned char next_char(const char **sp) {
    const unsigned char *s = (const unsigned char *)*sp;
    if (*s < 0x80) {
        *sp += 1;
        return *s;
    }
    uint32_t cp = 0;
    int n = *s >= 0xF0 ? 4 : *s >= 0xE0 ? 3 : *s >= 0xC0 ? 2 : 1;
    cp = n == 1 ? 0 : *s & (0x3F >> (n - 1));
    for (int i = 1; i < n; i++) {
        if ((s[i] & 0xC0) != 0x80) {
            n = i;
            break;
        }
        cp = cp << 6 | (s[i] & 0x3F);
    }
    *sp += n;
    switch (cp) {
    case 0x2013: case 0x2014: case 0x2212: return '-';
    case 0xD7: return 'x';
    case 0x2022: case 0xB7: return '*';
    case 0x2018: case 0x2019: return '\'';
    case 0x201C: case 0x201D: return '"';
    case 0x2026: return '.';
    default: return '?';
    }
}

static void text(int x, int y, const char *s, int maxw, uint32_t c, const rx_display_rect_t *r) {
    while (*s && maxw >= 8) {
        const uint8_t *g = font8x16_glyph(next_char(&s));
        for (int row = 0; row < 16; row++) {
            int yy = y + row;
            if (yy < (int)r->y || yy >= (int)(r->y + r->h)) continue;
            for (int b = 0; b < 8; b++) {
                int xx = x + b;
                if ((g[row] & (0x80 >> b)) && xx >= (int)r->x && xx < (int)(r->x + r->w)) fbp[(uint64_t)yy * W + xx] = c;
            }
        }
        x += 8;
        maxw -= 8;
    }
}

// The title bar's buttons, right to left: close, maximise, minimise
static void button_rect(const window_t *w, int k, int *x, int *y) {
    *x = w->x + (int)w->w - (k + 1) * (BTN + 4);
    *y = w->y - TITLE_H + (TITLE_H - BTN) / 2;
}

static void draw_window(const window_t *w, int focused, const rx_display_rect_t *r) {
    if (!w->fullscreen && !w->panel) {
        int fx, fy, fw, fh;
        frame_rect(w, &fx, &fy, &fw, &fh);
        uint32_t bc = focused ? C_BORDER : C_BORDER_OFF;
        fill(fx, fy, fw, BORDER, bc, r);
        fill(fx, fy + fh - BORDER, fw, BORDER, bc, r);
        fill(fx, fy, BORDER, fh, bc, r);
        fill(fx + fw - BORDER, fy, BORDER, fh, bc, r);
        fill(w->x, w->y - TITLE_H, (int)w->w, TITLE_H, focused ? C_TITLE : C_TITLE_OFF, r);
        int bx, by;
        for (int k = 0; k < 3; k++) {
            button_rect(w, k, &bx, &by);
            if (bx < w->x + 8) break;
            uint32_t c = focused ? (k == 0 ? C_CLOSE : C_BUTTON) : C_BORDER_OFF;
            fill(bx, by, BTN, BTN, c, r);
            // The symbols: x, a square, a line
            uint32_t ink = focused ? C_TEXT : C_TEXT_OFF;
            if (k == 0)
                for (int i = 4; i < BTN - 4; i++) {
                    fill(bx + i, by + i, 1, 1, ink, r);
                    fill(bx + BTN - 1 - i, by + i, 1, 1, ink, r);
                }
            else if (k == 1) {
                fill(bx + 4, by + 4, BTN - 8, 1, ink, r);
                fill(bx + 4, by + BTN - 5, BTN - 8, 1, ink, r);
                fill(bx + 4, by + 4, 1, BTN - 8, ink, r);
                fill(bx + BTN - 5, by + 4, 1, BTN - 8, ink, r);
            } else
                fill(bx + 4, by + BTN - 6, BTN - 8, 2, ink, r);
        }
        int room = (int)w->w - 3 * (BTN + 4) - 16;
        text(w->x + 8, w->y - TITLE_H + (TITLE_H - 16) / 2, w->title, room, focused ? C_TEXT : C_TEXT_OFF, r);
    }
    // The content
    int x0 = w->x > (int)r->x ? w->x : (int)r->x, y0 = w->y > (int)r->y ? w->y : (int)r->y;
    int x1 = w->x + (int)w->w < (int)(r->x + r->w) ? w->x + (int)w->w : (int)(r->x + r->w);
    int y1 = w->y + (int)w->h < (int)(r->y + r->h) ? w->y + (int)w->h : (int)(r->y + r->h);
    if (x1 <= x0) return;
    for (int yy = y0; yy < y1; yy++)
        memcpy(fbp + (uint64_t)yy * W + x0, w->pix + (uint64_t)(yy - w->y) * w->w + (x0 - w->x), (size_t)(x1 - x0) * 4);
}

static void draw_cursor(const rx_display_rect_t *r) {
    for (int row = 0; row < ARROW_H; row++)
        for (int k = 0; arrow[row][k]; k++) {
            int x = cx + k, y = cy + row;
            if (arrow[row][k] == ' ' || x < (int)r->x || x >= (int)(r->x + r->w) || y < (int)r->y || y >= (int)(r->y + r->h))
                continue;
            fbp[(uint64_t)y * W + x] = arrow[row][k] == '1' ? 0x000000 : 0xFFFFFF;
        }
}

// The wallpaper: a vertical Dracula gradient with a soft purple glow low
// on the left (computed once)
static uint32_t *wallpaper;

static void make_wallpaper(void) {
    wallpaper = malloc((size_t)W * H * 4);
    if (!wallpaper) return;
    // Ordered dithering (4x4 Bayer) keeps the gradient free of bands
    static const int bayer[4][4] = { { 0, 8, 2, 10 }, { 12, 4, 14, 6 }, { 3, 11, 1, 9 }, { 15, 7, 13, 5 } };
    int64_t rad = (int64_t)H * 10 / 14, r2 = rad * rad;
    for (uint32_t y = 0; y < H; y++) {
        // 0x1E1F29 at the top to 0x2B2D3C at the bottom, in 1/256ths
        int64_t t = (int64_t)y * 256 / H;
        int64_t br = 0x1E * 256 + (0x2B - 0x1E) * t, bg = 0x1F * 256 + (0x2D - 0x1F) * t, bb = 0x29 * 256 + (0x3C - 0x29) * t;
        for (uint32_t x = 0; x < W; x++) {
            // The glow: centre at (W/4, 3H/4), strength up to 16% of the accent
            int64_t dx = (int64_t)x - W / 4, dy = (int64_t)y - 3 * H / 4, d2 = dx * dx + dy * dy;
            int64_t g = d2 < r2 ? 41 * (r2 - d2) / r2 : 0; // 0..41 of 256
            g = g * (r2 - d2 > 0 ? (r2 - d2) : 0) / (r2 ? r2 : 1); // eased: falls off smoothly
            int64_t r = br + ((0xBD * 256 - br) * g) / 256, gg = bg + ((0x93 * 256 - bg) * g) / 256,
                    b = bb + ((0xF9 * 256 - bb) * g) / 256;
            int dth = bayer[y & 3][x & 3] * 16 + 8; // 0..255
            wallpaper[(uint64_t)y * W + x] = (uint32_t)((r + dth) >> 8) << 16 | (uint32_t)((gg + dth) >> 8) << 8 |
                                             (uint32_t)((b + dth) >> 8);
        }
    }
}

static void draw_wallpaper(const rx_display_rect_t *r) {
    if (!wallpaper) {
        fill((int)r->x, (int)r->y, (int)r->w, (int)r->h, C_DESKTOP, r);
        return;
    }
    for (uint32_t y = r->y; y < r->y + r->h; y++)
        memcpy(fbp + (uint64_t)y * W + r->x, wallpaper + (uint64_t)y * W + r->x, (size_t)r->w * 4);
}

// The lock screen's box
#define LOCK_W 360
#define LOCK_H 150
static void draw_lock(const rx_display_rect_t *r) {
    int x = ((int)W - LOCK_W) / 2, y = ((int)H - LOCK_H) / 2;
    fill(x, y, LOCK_W, LOCK_H, 0x343746, r);
    fill(x, y, LOCK_W, 2, C_BORDER, r);
    char line[96];
    snprintf(line, sizeof line, "Locked: %s", session_user);
    text(x + 24, y + 22, line, LOCK_W - 48, C_TEXT, r);
    text(x + 24, y + 48, "Type your password and Enter", LOCK_W - 48, C_TEXT_OFF, r);
    fill(x + 24, y + 78, LOCK_W - 48, 28, 0x282A36, r);
    char dots[65];
    size_t n = strlen(lock_pw);
    for (size_t i = 0; i < n; i++) dots[i] = '*';
    dots[n] = 0;
    text(x + 32, y + 84, dots, LOCK_W - 64, C_TEXT, r);
    if (lock_failed) text(x + 24, y + 116, "Wrong password", LOCK_W - 48, C_CLOSE, r);
}

#define TOAST_W 320
#define TOAST_H 48
static void toast_rect(int i, int *x, int *y) {
    *x = (int)W - TOAST_W - 16;
    *y = 16 + i * (TOAST_H + 8);
}

static void draw_toasts(const rx_display_rect_t *r) {
    for (int i = 0; i < ntoasts; i++) {
        int x, y;
        toast_rect(i, &x, &y);
        fill(x, y, TOAST_W, TOAST_H, 0x44475A, r);
        fill(x, y, 3, TOAST_H, C_BORDER, r);
        text(x + 14, y + 16, toasts[i].text, TOAST_W - 24, C_TEXT, r);
    }
}

static void compose(void) {
    if (!ndamage) return;
    for (int i = 0; i < ndamage; i++) {
        const rx_display_rect_t *r = &damage[i];
        draw_wallpaper(r);
        if (locked) {
            draw_lock(r);
        } else {
            for (int k = 0; k < norder; k++) {
                const window_t *w = &wins[order[k]];
                if (w->minimized || !w->pix) continue;
                draw_window(w, order[k] == focus, r);
            }
            draw_toasts(r);
        }
        draw_cursor(r);
    }
    int n = ndamage;
    ndamage = 0;
    int shown = rx_display_present(&d, damage, n);
    if (shown < 0) {
        printf("compositor: the display service went away\n");
        exit(1);
    }
    // Every window that committed: its frame is on the screen (or would be)
    for (int k = 0; k < MAX_WINDOWS; k++) {
        window_t *w = &wins[k];
        if (!w->used || !w->committed) continue;
        w->committed = 0;
        rx_window_event_t ev;
        memset(&ev, 0, sizeof ev);
        ev.op = RX_WINDOW_EV_FRAME;
        rx_channel_write(w->ch, 0, &ev, sizeof ev, 0, 0);
    }
}

// ---- windows ----

static void send_event(window_t *w, rx_window_event_t *ev) {
    ev->time = rx_clock_get_monotonic();
    rx_channel_write(w->ch, 0, ev, sizeof *ev, 0, 0);
}

static void send_simple(window_t *w, uint32_t op, uint32_t flags) {
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = op;
    ev.flags = flags;
    send_event(w, &ev);
}

static void send_state(window_t *w) {
    send_simple(w, RX_WINDOW_EV_STATE, (w->maximized ? RX_WINDOW_STATE_MAXIMIZED : 0) |
                                           (w->fullscreen ? RX_WINDOW_STATE_FULLSCREEN : 0) |
                                           (w->minimized ? RX_WINDOW_STATE_MINIMIZED : 0));
}

// Tell the panels about every window (step F6): a clearing event (x -1),
// then one per window: id, title (15 characters), focused, minimised
static void notify_panels(void) {
    for (int p = 0; p < MAX_WINDOWS; p++) {
        if (!wins[p].used || !wins[p].panel) continue;
        rx_window_event_t ev;
        memset(&ev, 0, sizeof ev);
        ev.op = RX_WINDOW_EV_WINDOW_INFO;
        ev.x = -1;
        send_event(&wins[p], &ev);
        for (int i = 0; i < norder; i++) {
            const window_t *w = &wins[order[i]];
            if (w->panel) continue;
            memset(&ev, 0, sizeof ev);
            ev.op = RX_WINDOW_EV_WINDOW_INFO;
            ev.x = order[i];
            ev.flags = (order[i] == focus ? 1u : 0u) | (w->minimized ? 2u : 0u);
            snprintf(ev.text, sizeof ev.text, "%s", w->title);
            send_event(&wins[p], &ev);
        }
    }
}

static void set_focus(int k) {
    if (k >= 0 && wins[k].panel) return; // the panel never takes the focus
    if (k == focus) return;
    if (focus >= 0 && wins[focus].used) {
        send_simple(&wins[focus], RX_WINDOW_EV_FOCUS, 0);
        damage_window(&wins[focus]);
    }
    focus = k;
    if (k >= 0) {
        send_simple(&wins[k], RX_WINDOW_EV_FOCUS, 1);
        damage_window(&wins[k]);
        vlog("focus \"%s\"\n", wins[k].title);
    }
    notify_panels();
}

static void raise(int k) {
    int i = 0;
    while (i < norder && order[i] != k) i++;
    if (i == norder) return;
    for (; i < norder - 1; i++) order[i] = order[i + 1];
    order[norder - 1] = k;
    damage_window(&wins[k]);
    // Panels stay on top
    for (int j = 0, n = norder; j < n; j++) {
        int p = order[j];
        if (!wins[p].panel || p == k) continue;
        for (int m = j; m < norder - 1; m++) order[m] = order[m + 1];
        order[norder - 1] = p;
        j--;
        n--;
    }
}

// The topmost window that is not minimised (nor a panel), or -1
static int top_window(void) {
    for (int i = norder - 1; i >= 0; i--)
        if (!wins[order[i]].minimized && !wins[order[i]].panel) return order[i];
    return -1;
}

static void unmap_client(window_t *w) {
    if (w->client) rx_vmar_unmap(rx_vmar_root, (uint64_t)w->client, w->client_size);
    if (w->vmo) rx_handle_close(w->vmo);
    w->client = 0;
    w->vmo = 0;
}

// Take a client's buffer of w x h; 0 or a negative errno
static int take_buffer(window_t *w, rx_handle_t vmo, uint32_t bw, uint32_t bh) {
    uint64_t size = ((uint64_t)bw * bh * 4 + 4095) & ~4095ull, vsize = 0, addr = 0;
    if (!bw || !bh || bw > 16384 || bh > 16384 || rx_vmo_get_size(vmo, &vsize) != RX_OK || vsize < size ||
        rx_vmar_map(rx_vmar_root, RX_VM_PERM_READ, 0, vmo, 0, size, &addr) != RX_OK) {
        rx_handle_close(vmo);
        return -EINVAL;
    }
    uint32_t *pix = calloc((size_t)bw * bh, 4);
    if (!pix) {
        rx_vmar_unmap(rx_vmar_root, addr, size);
        rx_handle_close(vmo);
        return -ENOMEM;
    }
    unmap_client(w);
    damage_window(w);           // where it was
    free(w->pix);
    w->pix = pix;
    w->vmo = vmo;
    w->client = (const uint32_t *)addr;
    w->client_size = size;
    w->w = bw;
    w->h = bh;
    damage_window(w);           // where it is now
    return 0;
}

static void remove_window(int k, const char *why) {
    window_t *w = &wins[k];
    vlog("window \"%s\" removed (%s)\n", w->title, why);
    damage_window(w);
    unmap_client(w);
    free(w->pix);
    rx_handle_close(w->ch);
    int i = 0, j = 0;
    for (; i < norder; i++)
        if (order[i] != k) order[j++] = order[i];
    norder = j;
    int was_panel = w->panel;
    memset(w, 0, sizeof *w);
    if (was_panel) panel_h = 0;
    if (focus == k) {
        focus = -1;
        set_focus(top_window());
    }
    notify_panels();
}

// Ask the client for a new size (and remember where the window goes)
static void configure(window_t *w, uint32_t nw, uint32_t nh) {
    if (nw < MIN_W) nw = MIN_W;
    if (nh < MIN_H) nh = MIN_H;
    if (nw == w->w && nh == w->h) return;
    w->want_w = nw;
    w->want_h = nh;
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = RX_WINDOW_EV_CONFIGURE;
    ev.w = nw;
    ev.h = nh;
    send_event(w, &ev);
}

static void save_geometry(window_t *w) {
    w->sx = w->x;
    w->sy = w->y;
    w->sw = w->w;
    w->sh = w->h;
}

static void restore_geometry(window_t *w) {
    damage_window(w);
    w->x = w->sx;
    w->y = w->sy;
    configure(w, w->sw, w->sh);
    damage_window(w);
}

static void toggle_maximize(int k) {
    window_t *w = &wins[k];
    if (w->fullscreen) return;
    if (w->maximized) {
        w->maximized = 0;
        restore_geometry(w);
    } else {
        save_geometry(w);
        w->maximized = 1;
        damage_window(w);
        w->x = BORDER;
        w->y = TITLE_H + BORDER;
        configure(w, W - 2 * BORDER, H - (uint32_t)panel_h - TITLE_H - 2 * BORDER);
    }
    vlog("window \"%s\" %s\n", w->title, w->maximized ? "maximized" : "restored");
    send_state(w);
}

static void set_fullscreen(int k, int on) {
    window_t *w = &wins[k];
    if (on == w->fullscreen) return;
    if (on) {
        if (!w->maximized) save_geometry(w);
        w->fullscreen = 1;
        damage_window(w);
        w->x = 0;
        w->y = 0;
        configure(w, W, H);
    } else {
        w->fullscreen = 0;
        if (w->maximized) {
            w->x = BORDER;
            w->y = TITLE_H + BORDER;
            configure(w, W - 2 * BORDER, H - (uint32_t)panel_h - TITLE_H - 2 * BORDER);
        } else {
            restore_geometry(w);
        }
    }
    damage_window(w);
    vlog("window \"%s\" full screen %s\n", w->title, on ? "on" : "off");
    send_state(w);
}

static void minimize(int k) {
    window_t *w = &wins[k];
    w->minimized = 1;
    damage_window(w);
    vlog("window \"%s\" minimized\n", w->title);
    send_state(w);
    notify_panels();
    if (focus == k) {
        focus = -1;
        send_simple(w, RX_WINDOW_EV_FOCUS, 0);
        set_focus(top_window());
    }
}

static void request_close(int k) {
    window_t *w = &wins[k];
    send_simple(w, RX_WINDOW_EV_CLOSE_REQUEST, 0);
    if (!w->close_deadline) w->close_deadline = rx_clock_get_monotonic() + 5000000000ull;
    vlog("asked \"%s\" to close\n", w->title);
}

// ---- client requests ----

static int new_window(rx_handle_t ch, const rx_window_req_t *r, const char *payload, uint32_t plen, rx_handle_t vmo) {
    int k = 0;
    while (k < MAX_WINDOWS && wins[k].used) k++;
    if (k == MAX_WINDOWS) return -ENOSPC;
    window_t *w = &wins[k];
    memset(w, 0, sizeof *w);
    size_t tl = plen < RX_WINDOW_MAX_TITLE ? plen : RX_WINDOW_MAX_TITLE;
    memcpy(w->title, payload, tl);
    w->title[tl] = 0;
    // Where: as asked, or cascading from the top left
    static int cascade;
    if (r->x >= 0 && r->y >= 0) {
        w->x = r->x;
        w->y = r->y;
    } else {
        w->x = 40 + (cascade % 10) * 32;
        w->y = 40 + TITLE_H + (cascade % 10) * 32;
        cascade++;
    }
    if (take_buffer(w, vmo, r->w, r->h) < 0) return -EINVAL;
    w->used = 1;
    w->ch = ch;
    order[norder++] = k;
    vlog("window \"%s\" %ux%u at %d,%d\n", w->title, w->w, w->h, w->x, w->y);
    raise(k); // under the panels
    return k;
}

// Copy committed rectangles from the client's buffer
static void commit(window_t *w, const rx_window_rect_t *rects, uint32_t n) {
    rx_window_rect_t all = { 0, 0, w->w, w->h };
    if (!n) {
        rects = &all;
        n = 1;
    }
    for (uint32_t i = 0; i < n; i++) {
        rx_window_rect_t r = rects[i];
        if (r.x >= w->w || r.y >= w->h) continue;
        if (r.w > w->w - r.x) r.w = w->w - r.x;
        if (r.h > w->h - r.y) r.h = w->h - r.y;
        for (uint32_t y = r.y; y < r.y + r.h; y++)
            memcpy(w->pix + (uint64_t)y * w->w + r.x, w->client + (uint64_t)y * w->w + r.x, (size_t)r.w * 4);
        if (!w->minimized) add_damage(w->x + (int)r.x, w->y + (int)r.y, (int)r.w, (int)r.h);
    }
    w->committed = 1;
}

// A notification (top right, 5 s; the oldest goes when there are three)
static void notify(const char *t, uint32_t n) {
    if (ntoasts == MAX_TOASTS) {
        memmove(&toasts[0], &toasts[1], sizeof toasts[0] * (MAX_TOASTS - 1));
        ntoasts--;
    }
    size_t c = n < sizeof toasts[0].text - 1 ? n : sizeof toasts[0].text - 1;
    memcpy(toasts[ntoasts].text, t, c);
    toasts[ntoasts].text[c] = 0;
    toasts[ntoasts].until = rx_clock_get_monotonic() + 5000000000ull;
    ntoasts++;
    add_damage((int)W - TOAST_W - 16, 16, TOAST_W, MAX_TOASTS * (TOAST_H + 8));
    vlog("notification \"%s\"\n", toasts[ntoasts - 1].text);
}

static void expire_toasts(uint64_t now) {
    int j = 0;
    for (int i = 0; i < ntoasts; i++)
        if (toasts[i].until > now) toasts[j++] = toasts[i];
    if (j != ntoasts) add_damage((int)W - TOAST_W - 16, 16, TOAST_W, MAX_TOASTS * (TOAST_H + 8));
    ntoasts = j;
}

static void lock_screen(void) {
    if (!session_user[0] || locked) return;
    if (udb_locked(session_user)) {
        // No password (the live image's root): nothing could unlock it
        static const char msg[] = "No password to unlock with: set one with passwd to lock the screen";
        notify(msg, sizeof msg - 1);
        return;
    }
    locked = 1;
    lock_pw[0] = 0;
    lock_failed = 0;
    add_damage(0, 0, (int)W, (int)H);
    vlog("locked\n");
}

// A key while locked: the password, Enter checks it
static void lock_key(const rx_input_event_t *e) {
    if (!(e->flags & RX_INPUT_PRESSED)) return;
    size_t n = strlen(lock_pw);
    if (e->code == 0x28 || e->code == 0x58) {
        int ok = udb_check(session_user, lock_pw);
        memset(lock_pw, 0, sizeof lock_pw);
        if (ok) {
            locked = 0;
            vlog("unlocked\n");
        } else {
            lock_failed = 1;
            vlog("wrong password at the lock screen\n");
        }
        add_damage(0, 0, (int)W, (int)H);
        return;
    }
    if (e->code == 0x2A) {
        if (n) lock_pw[n - 1] = 0;
    } else if (e->code == 0x29) {
        lock_pw[0] = 0;
    } else if (e->text[0] && !e->text[1] && (unsigned char)e->text[0] >= ' ' && n < sizeof lock_pw - 1) {
        lock_pw[n] = e->text[0];
        lock_pw[n + 1] = 0;
    }
    lock_failed = 0;
    add_damage(((int)W - LOCK_W) / 2, ((int)H - LOCK_H) / 2, LOCK_W, LOCK_H);
}

// A client channel's messages: 0 if the client is gone
static int client_messages(int k) {
    static char buf[sizeof(rx_window_req_t) + RX_WINDOW_MAX_CLIPBOARD];
    for (int count = 0; count < 64; count++) {
        window_t *w = &wins[k];
        uint32_t nb = 0, nh = 0;
        rx_handle_t h = 0;
        rx_status_t s = rx_channel_read(w->ch, 0, buf, &h, sizeof buf, 1, &nb, &nh);
        if (s == RX_ERR_SHOULD_WAIT) return 1;
        if (s != RX_OK) return 0;
        if (nb < sizeof(rx_window_req_t)) {
            if (nh) rx_handle_close(h);
            continue;
        }
        rx_window_req_t r;
        memcpy(&r, buf, sizeof r);
        const char *payload = buf + sizeof r;
        uint32_t plen = nb - (uint32_t)sizeof r;
        switch (r.op) {
        case RX_WINDOW_COMMIT:
            if (plen >= r.n * sizeof(rx_window_rect_t) && r.n <= RX_WINDOW_MAX_RECTS)
                commit(w, (const rx_window_rect_t *)payload, r.n);
            break;
        case RX_WINDOW_BUFFER:
            if (nh) {
                nh = 0;
                if (take_buffer(w, h, r.w, r.h) == 0) w->want_w = w->want_h = 0;
            }
            break;
        case RX_WINDOW_SET_TITLE: {
            size_t tl = plen < RX_WINDOW_MAX_TITLE ? plen : RX_WINDOW_MAX_TITLE;
            memcpy(w->title, payload, tl);
            w->title[tl] = 0;
            add_damage(w->x, w->y - TITLE_H, (int)w->w, TITLE_H);
            notify_panels();
            break;
        }
        case RX_WINDOW_CLIPBOARD_SET:
            clipboard_len = plen < RX_WINDOW_MAX_CLIPBOARD ? plen : RX_WINDOW_MAX_CLIPBOARD;
            memcpy(clipboard, payload, clipboard_len);
            clipboard[clipboard_len] = 0;
            vlog("clipboard: %zu bytes from \"%s\"\n", clipboard_len, w->title);
            break;
        case RX_WINDOW_CLIPBOARD_GET: {
            static char out[sizeof(rx_window_event_t) + RX_WINDOW_MAX_CLIPBOARD];
            rx_window_event_t ev;
            memset(&ev, 0, sizeof ev);
            ev.op = RX_WINDOW_CLIPBOARD_GET | RX_WINDOW_REPLY;
            ev.n = (uint32_t)clipboard_len;
            memcpy(out, &ev, sizeof ev);
            memcpy(out + sizeof ev, clipboard, clipboard_len);
            rx_channel_write(w->ch, 0, out, (uint32_t)(sizeof ev + clipboard_len), 0, 0);
            break;
        }
        case RX_WINDOW_SET_FULLSCREEN:
            if (!w->panel) set_fullscreen(k, r.flags != 0);
            break;
        case RX_WINDOW_SET_ROLE:
            if (r.flags == RX_WINDOW_ROLE_PANEL && !w->panel) {
                // Docked at the bottom, the screen's width, on top
                damage_window(w);
                if (focus == k) {
                    focus = -1;
                    set_focus(top_window());
                }
                w->panel = 1;
                panel_h = (int)w->h;
                w->x = 0;
                w->y = (int)H - panel_h;
                configure(w, W, w->h);
                raise(k);
                damage_window(w);
                vlog("window \"%s\" is the panel\n", w->title);
                notify_panels();
            }
            break;
        case RX_WINDOW_ACTIVATE:
            if (w->panel && r.x >= 0 && r.x < MAX_WINDOWS && wins[r.x].used && !wins[r.x].panel) {
                window_t *t = &wins[r.x];
                if (t->minimized) {
                    t->minimized = 0;
                    send_state(t);
                    raise(r.x);
                    set_focus(r.x);
                } else if (focus == r.x) {
                    minimize(r.x);
                } else {
                    raise(r.x);
                    set_focus(r.x);
                }
                damage_window(t);
                notify_panels();
            }
            break;
        case RX_WINDOW_NOTIFY:
            notify(payload, plen);
            break;
        case RX_WINDOW_LOCK:
            lock_screen();
            break;
        default:
            break;
        }
        if (nh) rx_handle_close(h);
    }
    return 1;
}

// A new connection: its first message must be CREATE
#define MAX_PENDING 16
static rx_handle_t pending[MAX_PENDING];
static int npending;

static int pending_message(int i) {
    static char buf[sizeof(rx_window_req_t) + RX_WINDOW_MAX_CLIPBOARD];
    rx_handle_t ch = pending[i];
    uint32_t nb = 0, nh = 0;
    rx_handle_t h = 0;
    rx_status_t s = rx_channel_read(ch, 0, buf, &h, sizeof buf, 1, &nb, &nh);
    if (s == RX_ERR_SHOULD_WAIT) return 1;
    pending[i] = pending[--npending];
    if (s != RX_OK) {
        rx_handle_close(ch);
        return 0;
    }
    rx_window_req_t r;
    memcpy(&r, buf, sizeof r);
    if (nb >= sizeof r && r.op == RX_WINDOW_NOTIFY) {
        // A notification without a window (the notify command)
        notify(buf + sizeof r, nb - (uint32_t)sizeof r);
        if (nh) rx_handle_close(h);
        rx_handle_close(ch);
        return 0;
    }
    int k = -EINVAL;
    if (nb >= sizeof r && r.op == RX_WINDOW_CREATE && nh) {
        k = new_window(ch, &r, buf + sizeof r, nb - (uint32_t)sizeof r, h);
        nh = 0;
    }
    if (nh) rx_handle_close(h);
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = RX_WINDOW_CREATE | RX_WINDOW_REPLY;
    ev.status = k < 0 ? k : 0;
    if (k >= 0) {
        ev.w = wins[k].w;
        ev.h = wins[k].h;
        ev.x = wins[k].x;
        ev.y = wins[k].y;
    }
    rx_channel_write(ch, 0, &ev, sizeof ev, 0, 0);
    if (k < 0) {
        rx_handle_close(ch);
        return 0;
    }
    set_focus(k);
    return 1;
}

// ---- input ----

enum { DRAG_NONE, DRAG_MOVE, DRAG_RESIZE };
static int drag, drag_win, drag_dx, drag_dy, drag_edges;  // edges: 1 right, 2 bottom
static int grab_win = -1;       // the window a button went down in
static int pointer_in = -1;     // the window that last got a pointer event
static uint64_t last_click;
static int last_click_win = -1;

enum { HIT_NONE, HIT_CONTENT, HIT_TITLE, HIT_CLOSE, HIT_MAX, HIT_MIN, HIT_RESIZE };

static int hit(int px, int py, int *which, int *edges) {
    for (int i = norder - 1; i >= 0; i--) {
        int k = order[i];
        const window_t *w = &wins[k];
        if (w->minimized) continue;
        *which = k;
        if (px >= w->x && px < w->x + (int)w->w && py >= w->y && py < w->y + (int)w->h) return HIT_CONTENT;
        if (w->fullscreen || w->panel) continue;
        if (px >= w->x && px < w->x + (int)w->w && py >= w->y - TITLE_H && py < w->y) {
            for (int b = 0; b < 3; b++) {
                int bx, by;
                button_rect(w, b, &bx, &by);
                if (px >= bx && px < bx + BTN && py >= by && py < by + BTN) return b == 0 ? HIT_CLOSE : b == 1 ? HIT_MAX : HIT_MIN;
            }
            return HIT_TITLE;
        }
        // The resize zone: the right and bottom borders and a few pixels outside them
        int fx, fy, fw, fh;
        frame_rect(w, &fx, &fy, &fw, &fh);
        if (!w->maximized && px >= fx && py >= fy && px < fx + fw + GRIP && py < fy + fh + GRIP) {
            int e = (px >= fx + fw - BORDER ? 1 : 0) | (py >= fy + fh - BORDER ? 2 : 0);
            if (e) {
                *edges = e;
                return HIT_RESIZE;
            }
            if (px < fx + fw && py < fy + fh) return HIT_TITLE; // the border elsewhere
        }
    }
    *which = -1;
    return HIT_NONE;
}

static void pointer_to(int k, uint32_t flags, int32_t wheel) {
    window_t *w = &wins[k];
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = RX_WINDOW_EV_POINTER;
    ev.x = cx - w->x;
    ev.y = cy - w->y;
    ev.buttons = buttons;
    ev.wheel = wheel;
    ev.flags = flags;
    send_event(w, &ev);
}

static void move_cursor(int nx, int ny) {
    if (nx < 0) nx = 0;
    if (ny < 0) ny = 0;
    if (nx >= (int)W) nx = (int)W - 1;
    if (ny >= (int)H) ny = (int)H - 1;
    if (nx == cx && ny == cy) return;
    add_damage(cx, cy, ARROW_W, ARROW_H);
    cx = nx;
    cy = ny;
    add_damage(cx, cy, ARROW_W, ARROW_H);
}

static void pointer_event(const rx_input_event_t *e) {
    if (locked) {
        // The cursor moves; nothing else
        if (e->flags & RX_INPUT_ABSOLUTE)
            move_cursor((int)((uint64_t)e->x * (W - 1) / 32767), (int)((uint64_t)e->y * (H - 1) / 32767));
        else
            move_cursor(cx + e->x, cy + e->y);
        buttons = e->code;
        return;
    }
    int ox = cx, oy = cy;
    if (e->flags & RX_INPUT_ABSOLUTE)
        move_cursor((int)((uint64_t)e->x * (W - 1) / 32767), (int)((uint64_t)e->y * (H - 1) / 32767));
    else
        move_cursor(cx + e->x, cy + e->y);
    int moved = cx != ox || cy != oy;
    uint32_t old = buttons;
    buttons = e->code;
    uint32_t pressed = buttons & ~old, released = old & ~buttons;

    // A drag in progress
    if (drag != DRAG_NONE) {
        window_t *w = &wins[drag_win];
        if (moved && drag == DRAG_MOVE) {
            damage_window(w);
            w->x = cx - drag_dx;
            w->y = cy - drag_dy;
            if (w->y < TITLE_H + BORDER) w->y = TITLE_H + BORDER; // the title bar stays reachable
            damage_window(w);
        } else if (moved && drag == DRAG_RESIZE) {
            int nw = (drag_edges & 1) ? cx - w->x - drag_dx : (int)w->w;
            int nh = (drag_edges & 2) ? cy - w->y - drag_dy : (int)w->h;
            // One CONFIGURE at a time: the next when the client has answered
            if (!w->want_w && nw > 0 && nh > 0) configure(w, (uint32_t)nw, (uint32_t)nh);
        }
        if (!(buttons & 1)) {
            if (drag == DRAG_RESIZE) {
                int nw = (drag_edges & 1) ? cx - w->x - drag_dx : (int)w->w;
                int nh = (drag_edges & 2) ? cy - w->y - drag_dy : (int)w->h;
                if (nw > 0 && nh > 0) configure(w, (uint32_t)nw, (uint32_t)nh);
                vlog("window \"%s\" resized to %dx%d\n", w->title, nw < MIN_W ? MIN_W : nw, nh < MIN_H ? MIN_H : nh);
            } else {
                vlog("window \"%s\" moved to %d,%d\n", w->title, w->x, w->y);
            }
            drag = DRAG_NONE;
        }
        return;
    }

    // A button held: the window it went down in gets everything
    if (grab_win >= 0 && wins[grab_win].used) {
        uint32_t f = (moved ? RX_WINDOW_PTR_MOTION : 0) | (pressed || released ? RX_WINDOW_PTR_BUTTON : 0) |
                     (e->wheel ? RX_WINDOW_PTR_WHEEL : 0);
        if (f) pointer_to(grab_win, f, e->wheel);
        if (!buttons) grab_win = -1;
        return;
    }
    grab_win = -1;

    int k, edges = 0, where = hit(cx, cy, &k, &edges);
    if (pressed) {
        if (k >= 0 && !wins[k].panel) {
            raise(k);
            set_focus(k);
        }
        if (pressed & 1) {
            window_t *w = k >= 0 ? &wins[k] : 0;
            switch (where) {
            case HIT_TITLE: {
                uint64_t now = rx_clock_get_monotonic();
                if (last_click_win == k && now - last_click < 400000000ull) {
                    toggle_maximize(k);
                    last_click_win = -1;
                    return;
                }
                last_click = now;
                last_click_win = k;
                if (w->maximized) return;
                drag = DRAG_MOVE;
                drag_win = k;
                drag_dx = cx - w->x;
                drag_dy = cy - w->y;
                return;
            }
            case HIT_RESIZE:
                drag = DRAG_RESIZE;
                drag_win = k;
                drag_edges = edges;
                drag_dx = cx - w->x - (int)w->w;
                drag_dy = cy - w->y - (int)w->h;
                return;
            case HIT_CLOSE: request_close(k); return;
            case HIT_MAX: toggle_maximize(k); return;
            case HIT_MIN: minimize(k); return;
            default: break;
            }
        }
    }
    // Pointer events reach the focused window only, while over its content
    // (and the panel, which is never focused)
    if (where == HIT_CONTENT && (k == focus || wins[k].panel)) {
        if (pointer_in >= 0 && pointer_in != k && wins[pointer_in].used) pointer_to(pointer_in, RX_WINDOW_PTR_LEAVE, 0);
        uint32_t f = (moved ? RX_WINDOW_PTR_MOTION : 0) | (pressed || released ? RX_WINDOW_PTR_BUTTON : 0) |
                     (e->wheel ? RX_WINDOW_PTR_WHEEL : 0);
        if (f) pointer_to(k, f, e->wheel);
        pointer_in = k;
        if (pressed) grab_win = k;
    } else if (pointer_in >= 0) {
        if (wins[pointer_in].used) pointer_to(pointer_in, RX_WINDOW_PTR_LEAVE, 0);
        pointer_in = -1;
    }
}

// Alt+Tab: the bottom-most window other than the focused one comes to
// the top and gets the focus (a minimised one comes back): repeated, it
// goes through all the windows
static void cycle_focus(void) {
    int k = -1;
    for (int i = 0; i < norder; i++)
        if (order[i] != focus) {
            k = order[i];
            break;
        }
    if (k < 0) return;
    if (wins[k].minimized) {
        wins[k].minimized = 0;
        damage_window(&wins[k]);
        send_state(&wins[k]);
    }
    raise(k);
    set_focus(k);
}

static void key_event(const rx_input_event_t *e) {
    int pressed = e->flags & RX_INPUT_PRESSED;
    if (locked) {
        lock_key(e);
        return;
    }
    // Super+L (or Ctrl+Alt+L): lock
    if (pressed && e->code == 0x0F && ((e->mods & (RX_INPUT_MOD_LGUI | RX_INPUT_MOD_RGUI)) ||
                                       ((e->mods & (RX_INPUT_MOD_LCTRL | RX_INPUT_MOD_RCTRL)) && (e->mods & (RX_INPUT_MOD_LALT | RX_INPUT_MOD_RALT))))) {
        lock_screen();
        return;
    }
    int alt = e->mods & (RX_INPUT_MOD_LALT | RX_INPUT_MOD_RALT);
    if (pressed && alt && e->code == 0x2B) { // Alt+Tab
        cycle_focus();
        return;
    }
    if (pressed && alt && e->code == 0x3D) { // Alt+F4
        if (focus >= 0) request_close(focus);
        return;
    }
    if (pressed && e->code == 0x44 && !(e->flags & RX_INPUT_REPEAT)) { // F11
        if (focus >= 0) set_fullscreen(focus, !wins[focus].fullscreen);
        return;
    }
    if (focus < 0) return;
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = RX_WINDOW_EV_KEY;
    ev.code = e->code;
    ev.mods = e->mods;
    ev.flags = e->flags;
    memcpy(ev.text, e->text, sizeof ev.text < sizeof e->text ? sizeof ev.text : sizeof e->text);
    ev.text[sizeof ev.text - 1] = 0;
    send_event(&wins[focus], &ev);
}

static void input_events(void) {
    rx_input_event_t e;
    uint32_t nb = 0;
    for (int i = 0; i < 256; i++) {
        rx_status_t s = rx_channel_read(input_ch, 0, &e, 0, sizeof e, 0, &nb, 0);
        if (s == RX_ERR_SHOULD_WAIT) return;
        if (s != RX_OK) {
            // The input service was restarted: register with the new one
            rx_handle_close(input_ch);
            input_ch = 0;
            return;
        }
        if (nb != sizeof e) continue; // the GRAPHICS reply
        if (e.type == RX_INPUT_KEY) key_event(&e);
        else if (e.type == RX_INPUT_POINTER) pointer_event(&e);
    }
}

static void connect_input(void) {
    if (rx_service_connect("/svc/input", &input_ch) < 0) {
        input_ch = 0;
        return;
    }
    rx_input_req_t r = { RX_INPUT_OP_GRAPHICS, 0 };
    rx_channel_write(input_ch, 0, &r, sizeof r, 0, 0);
}

// ---- main ----

int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "-v")) verbose = 1;
    else if (argc != 1) {
        fprintf(stderr, "usage: compositor [-v]\n");
        return 2;
    }
    int r = rx_display_open(&d);
    if (r < 0) {
        fprintf(stderr, "compositor: no screen: %s\n", strerror(-r));
        return 1;
    }
    W = d.width;
    H = d.height;
    fbp = d.pixels;
    cx = (int)W / 2;
    cy = (int)H / 2;
    make_wallpaper();
    rx_handle_t control = rx_take_startup_handle(RX_PA_USER0); // from desktop --run, if it started us
    connect_input();
    rx_handle_t listen, theirs;
    if (rx_channel_create(0, &listen, &theirs) != RX_OK || rx_service_publish("/svc/window", theirs) < 0) {
        fprintf(stderr, "compositor: cannot publish /svc/window\n");
        return 1;
    }
    vlog("%ux%u, /svc/window\n", W, H);
    add_damage(0, 0, (int)W, (int)H);
    compose();
    for (;;) {
        rx_wait_item_t items[3 + MAX_WINDOWS + MAX_PENDING];
        int n = 0, idx[MAX_WINDOWS];
        int i_disp = n;
        items[n++] = (rx_wait_item_t){ d.ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        int i_lis = n;
        items[n++] = (rx_wait_item_t){ listen, RX_CHANNEL_READABLE, 0 };
        int i_in = -1;
        if (input_ch) { i_in = n; items[n++] = (rx_wait_item_t){ input_ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 }; }
        int nw = 0;
        uint64_t deadline = RX_TIME_INFINITE;
        for (int k = 0; k < MAX_WINDOWS; k++) {
            if (!wins[k].used) continue;
            idx[nw++] = k;
            items[n++] = (rx_wait_item_t){ wins[k].ch, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
            if (wins[k].close_deadline && wins[k].close_deadline < deadline) deadline = wins[k].close_deadline;
        }
        int i_pend = n;
        for (int i = 0; i < npending; i++) items[n++] = (rx_wait_item_t){ pending[i], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        int i_ctl = -1;
        if (control) { i_ctl = n; items[n++] = (rx_wait_item_t){ control, RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 }; }
        for (int i = 0; i < ntoasts; i++)
            if (toasts[i].until < deadline) deadline = toasts[i].until;
        // Without the input service, look for it again now and then
        if (!input_ch && deadline > rx_clock_get_monotonic() + 1000000000ull) deadline = rx_clock_get_monotonic() + 1000000000ull;
        rx_object_wait_many(items, (uint64_t)n, deadline);

        if (i_ctl >= 0 && items[i_ctl].pending) {
            // The desktop: "user NAME" or "user"; when it ends, so do we
            char msg[80];
            uint32_t nb = 0;
            rx_status_t cs;
            while ((cs = rx_channel_read(control, 0, msg, 0, sizeof msg - 1, 0, &nb, 0)) == RX_OK) {
                msg[nb] = 0;
                if (!strncmp(msg, "user", 4)) {
                    snprintf(session_user, sizeof session_user, "%s", msg[4] == ' ' ? msg + 5 : "");
                    vlog("session user \"%s\"\n", session_user);
                    if (!session_user[0] && locked) {
                        locked = 0;
                        add_damage(0, 0, (int)W, (int)H);
                    }
                }
            }
            if (cs == RX_ERR_PEER_CLOSED) return 0;
        }

        if (items[i_disp].pending) {
            rx_display_msg_t ev;
            int pr;
            while ((pr = rx_display_poll(&d, &ev)) > 0)
                if (ev.op == RX_DISPLAY_EV_VISIBLE) {
                    visible = (int)ev.a;
                    if (visible) add_damage(0, 0, (int)W, (int)H);
                }
            if (pr < 0) {
                printf("compositor: the display service went away\n");
                return 1;
            }
        }
        if (i_in >= 0 && items[i_in].pending) input_events();
        if (!input_ch) connect_input();
        for (int j = 0; j < nw; j++) {
            int k = idx[j];
            if (!wins[k].used || !items[i_lis + 1 + (i_in >= 0) + j].pending) continue;
            if (!client_messages(k)) remove_window(k, "its client closed it or ended");
        }
        for (int i = npending - 1; i >= 0; i--)
            if (items[i_pend + i].pending) pending_message(i);
        if (items[i_lis].pending & RX_CHANNEL_READABLE) {
            char buf[64];
            rx_handle_t h;
            uint32_t nb = 0, nh = 0;
            while (rx_channel_read(listen, 0, buf, &h, sizeof buf, 1, &nb, &nh) == RX_OK) {
                if (!nh) continue;
                if (npending == MAX_PENDING) rx_handle_close(h);
                else pending[npending++] = h;
            }
        }
        uint64_t now = rx_clock_get_monotonic();
        expire_toasts(now);
        for (int k = 0; k < MAX_WINDOWS; k++)
            if (wins[k].used && wins[k].close_deadline && now >= wins[k].close_deadline)
                remove_window(k, "it did not close when asked");
        compose();
    }
}
