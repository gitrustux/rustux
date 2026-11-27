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
// Appearance (2026-11-24; five themes and the user's own 2026-11-25): the
// theme and the wallpaper come from /etc/rustica/appearance and then the
// session user's ~/.config/rustica.conf (theme=: a theme file's name, the
// user's own ~/.config/rustica/themes/NAME.theme before the system's in
// /usr/share/rustica/themes; wallpaper=: rustica, dracula, forest, arctic,
// sakura, gradient, or a QOI file of theirs, which Settings makes),
// checked every second; every window gets the theme's colours when it is
// made and when they change (RX_WINDOW_EV_APPEARANCE). A right
// click on the desktop opens a small menu whose choices the panel carries
// out (RX_WINDOW_EV_DESKTOP).
//
// Root only (the display and input services give the screen and the
// input to root).

#include <errno.h>
#include <fcntl.h>
#include <pwd.h>
#include <sys/stat.h>
#include <unistd.h>
#include <rustux/display.h>
#include <rustux/input.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <rustux/window.h>
#include <rustux/sound.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "themefile.h"
#include "titlefont.h"
#include "userdb.h"

#define MAX_WINDOWS 32
#define TITLE_H 24      // title bar height
#define BORDER 1
#define GRIP 5          // resize zone outside the border
#define BTN 16          // title bar button size
#define MIN_W 64
#define MIN_H 32

// The theme (2026-11-25): a theme file (themefile.h, the same files as the
// toolkit's), and the colours of the title bars, menus and lock screen
// worked out from its tokens
typedef struct {
    const char *name;
    uint32_t desktop, title, title_off, text, text_off, border, border_off, close, button, surface, field, hover;
} ctheme_t;
static theme_t cur_theme;
static ctheme_t ctheme;
static const ctheme_t *th = &ctheme;
// What windows are sent with RX_WINDOW_EV_APPEARANCE
static char theme_blob[2048];
static int theme_blob_len;

static uint32_t cmix(uint32_t a, uint32_t b, int k256) {
    uint32_t r = 0;
    for (int s = 0; s <= 16; s += 8) {
        int x = (int)(a >> s & 255), y = (int)(b >> s & 255);
        r |= (uint32_t)(x + (y - x) * k256 / 256) << s;
    }
    return r;
}

static void use_theme(const theme_t *t) {
    cur_theme = *t;
    const uint32_t *c = cur_theme.c;
    ctheme = (ctheme_t){ cur_theme.name, c[TH_BG], c[TH_SURFACE], cmix(c[TH_SURFACE], c[TH_BG], 128), c[TH_TEXT],
                         c[TH_MUTED], c[TH_PRIMARY], c[TH_RAISED], c[TH_DANGER], c[TH_BORDER], c[TH_SURFACE], c[TH_BG],
                         c[TH_HOVER] };
    theme_blob_len = theme_text(&cur_theme, theme_blob, sizeof theme_blob);
}

#define C_DESKTOP     (th->desktop)
#define C_TITLE       (th->title)
#define C_TITLE_OFF   (th->title_off)
#define C_TEXT        (th->text)
#define C_TEXT_OFF    (th->text_off)
#define C_BORDER      (th->border)
#define C_BORDER_OFF  (th->border_off)
#define C_CLOSE       (th->close)
#define C_BUTTON      (th->button)

typedef struct {
    int used;
    rx_handle_t ch;
    char title[RX_WINDOW_MAX_TITLE + 1];
    uint32_t cursor;            // the pointer's shape over its content (RX_WINDOW_CURSOR_*)
    char app[64];               // the app it belongs to (SET_APP), for the panel's icons
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
    int panel;                  // the panel: no frame, docked at an edge, on top, never focused
    int tiled;                  // SNAP_*: filling a half or a quarter of the work area
    int desktop;                // the desktop's icons (step F12): the work area, no frame, below every window, blended over the wallpaper
} window_t;

static window_t wins[MAX_WINDOWS];
static int order[MAX_WINDOWS], norder;  // z-order, bottom first (indexes into wins)
static int focus = -1;
static int verbose;

static rx_display_t d;
static uint32_t W, H;
static uint32_t PW, PH; // the first head's part (the panel and maximised windows; step F8)
static rx_handle_t input_ch;
static int visible = 1;

static char clipboard[RX_WINDOW_MAX_CLIPBOARD + 1];
static size_t clipboard_len;

// The panel (step F10): its edge (RX_WINDOW_EDGE_*), its thickness, and
// auto-hide (then it takes no room and shows when the pointer reaches the
// edge)
static int panel_edge = RX_WINDOW_EDGE_BOTTOM, panel_size, panel_autohide, panel_shown = 1;

// What windows may use: the first head without the panel
static void work_area(int *x, int *y, int *w, int *h) {
    int r = panel_autohide ? 0 : panel_size;
    *x = 0; *y = 0; *w = (int)PW; *h = (int)PH;
    switch (panel_edge) {
    case RX_WINDOW_EDGE_TOP: *y = r; *h -= r; break;
    case RX_WINDOW_EDGE_LEFT: *x = r; *w -= r; break;
    case RX_WINDOW_EDGE_RIGHT: *w -= r; break;
    default: *h -= r; break;
    }
}

static void relayout(void);
static void toggle_maximize(int k);

// Tiling (step F13): the part of the work area a window snaps to
enum { SNAP_NONE, SNAP_LEFT, SNAP_RIGHT, SNAP_TL, SNAP_TR, SNAP_BL, SNAP_BR, SNAP_MAX };
static void snap_rect(int z, int *x, int *y, int *w, int *h) {
    int ax, ay, aw, ah;
    work_area(&ax, &ay, &aw, &ah);
    int hw = aw / 2, hh = ah / 2;
    *x = ax; *y = ay; *w = aw; *h = ah;
    switch (z) {
    case SNAP_LEFT: *w = hw; break;
    case SNAP_RIGHT: *x = ax + hw; *w = aw - hw; break;
    case SNAP_TL: *w = hw; *h = hh; break;
    case SNAP_TR: *x = ax + hw; *w = aw - hw; *h = hh; break;
    case SNAP_BL: *y = ay + hh; *w = hw; *h = ah - hh; break;
    case SNAP_BR: *x = ax + hw; *y = ay + hh; *w = aw - hw; *h = ah - hh; break;
    default: break;
    }
}
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
    if (w->fullscreen || w->panel || w->desktop) {
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
static int hwcursor;    // the display draws the pointer (virtio-gpu, step F8)
// The pointer's shapes (RX_WINDOW_CURSOR_*): '1' black, '2' white, ' '
// clear; drawn with the hot spot at the pointer (made by a script from
// masks: an outline around each shape)
typedef struct {
    const char **rows;
    int w, h, hx, hy;
} cursor_shape_t;
static const char *cur_arrow[] = {
    "1",
    "11",
    "121",
    "1221",
    "12221",
    "122221",
    "1222221",
    "12222221",
    "122222221",
    "1222222221",
    "12222211111",
    "1221221",
    "121 1221",
    "11  1221",
    "1    1221",
    "     1221",
    "      11",
    0,
};
static const char *cur_hand[] = {
    "     11",
    "    1221",
    "    1221",
    "    1221",
    "    1221",
    "    122111",
    "    1221221111",
    "    12212212211",
    " 11 122122122121",
    "1221122222222221",
    "1222122222222221",
    " 122222222222221",
    "  12222222222221",
    "  1222222222221",
    "   122222222221",
    "   12222222221",
    "    1222222221",
    "    1222222221",
    "    111111111",
    0,
};
static const char *cur_text[] = {
    "222222222",
    "211111112",
    "222212222",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "   212",
    "222212222",
    "211111112",
    "222222222",
    0,
};
static const char *cur_wait[] = {
    "11111111111",
    "12222222221",
    " 111111111",
    " 122222221",
    " 112222211",
    "  1122211",
    "   11211",
    "    121",
    "    121",
    "   11211",
    "  1122211",
    " 112212211",
    " 122222221",
    " 111111111",
    "12222222221",
    "11111111111",
    0,
};
static const char *cur_move[] = {
    "         111",
    "        11211",
    "       1122211",
    "      112222211",
    "      122222221",
    "      111121111",
    "   111   121   111",
    "  1121   121   1211",
    " 11221   121   12211",
    "112221111121111122211",
    "122222222222222222221",
    "112221111121111122211",
    " 11221   121   12211",
    "  1121   121   1211",
    "   111   121   111",
    "      111121111",
    "      122222221",
    "      112222211",
    "       1122211",
    "        11211",
    "         111",
    0,
};
static const char *cur_resize_h[] = {
    "    111       111",
    "   1121       1211",
    "  11221       12211",
    " 1122211111111122211",
    "112222222222222222211",
    "122222222222222222221",
    "112222222222222222211",
    " 1122211111111122211",
    "  11221       12211",
    "   1121       1211",
    "    111       111",
    0,
};
static const char *cur_resize_v[] = {
    "    111",
    "   11211",
    "  1122211",
    " 112222211",
    "11222222211",
    "12222222221",
    "11112221111",
    "   12221",
    "   12221",
    "   12221",
    "   12221",
    "   12221",
    "   12221",
    "   12221",
    "11112221111",
    "12222222221",
    "11222222211",
    " 112222211",
    "  1122211",
    "   11211",
    "    111",
    0,
};
static const char *cur_resize_nwse[] = {
    "",
    "   111",
    "  11211111",
    " 112222221",
    " 122222211",
    " 112222211",
    "  122222211",
    "  1222222211",
    "  12112222211",
    "  111112222211111",
    "      11222221121",
    "       1122222221",
    "        112222221",
    "         112222211",
    "         112222221",
    "         122222211",
    "         11111211",
    "             111",
    "",
    0,
};
static const char *cur_resize_nesw[] = {
    "",
    "             111",
    "         11111211",
    "         122222211",
    "         112222221",
    "         112222211",
    "        112222221",
    "       1122222221",
    "      11222221121",
    "  111112222211111",
    "  12112222211",
    "  1222222211",
    "  122222211",
    " 112222211",
    " 122222211",
    " 112222221",
    "  11211111",
    "   111",
    "",
    0,
};
static const cursor_shape_t shapes[RX_WINDOW_CURSOR_COUNT] = {
    [RX_WINDOW_CURSOR_ARROW] = {cur_arrow, 11, 17, 0, 0},
    [RX_WINDOW_CURSOR_HAND] = {cur_hand, 16, 19, 5, 0},
    [RX_WINDOW_CURSOR_TEXT] = {cur_text, 9, 18, 4, 9},
    [RX_WINDOW_CURSOR_WAIT] = {cur_wait, 11, 16, 5, 8},
    [RX_WINDOW_CURSOR_MOVE] = {cur_move, 21, 21, 10, 10},
    [RX_WINDOW_CURSOR_RESIZE_H] = {cur_resize_h, 21, 11, 10, 5},
    [RX_WINDOW_CURSOR_RESIZE_V] = {cur_resize_v, 11, 21, 5, 10},
    [RX_WINDOW_CURSOR_RESIZE_NWSE] = {cur_resize_nwse, 18, 19, 9, 9},
    [RX_WINDOW_CURSOR_RESIZE_NESW] = {cur_resize_nesw, 18, 19, 9, 9},
};
static uint32_t cur_shape;  // the one shown
static int upload_hwcursor(void);

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

// One UTF-8 code point (an invalid byte counts as itself)
static uint32_t next_cp(const char **sp) {
    const unsigned char *s = (const unsigned char *)*sp;
    if (*s < 0x80) {
        *sp += 1;
        return *s;
    }
    int n = *s >= 0xF0 ? 4 : *s >= 0xE0 ? 3 : *s >= 0xC0 ? 2 : 1;
    uint32_t cp = n == 1 ? *s : (uint32_t)(*s & (0x3F >> (n - 1)));
    for (int i = 1; i < n; i++) {
        if ((s[i] & 0xC0) != 0x80) {
            n = i;
            break;
        }
        cp = cp << 6 | (s[i] & 0x3F);
    }
    *sp += n;
    return cp;
}

static const titlefont_glyph_t *title_glyph(uint32_t cp) {
    int lo = 0, hi = (int)(sizeof titlefont_glyphs / sizeof titlefont_glyphs[0]) - 1;
    while (lo <= hi) {
        int mid = (lo + hi) / 2;
        if (titlefont_glyphs[mid].cp == cp) return &titlefont_glyphs[mid];
        if (titlefont_glyphs[mid].cp < cp) lo = mid + 1;
        else hi = mid - 1;
    }
    return cp == '?' ? 0 : title_glyph('?');
}

static uint32_t blend(uint32_t bg, uint32_t fg, unsigned a) {
    uint32_t out = 0;
    for (int sh = 0; sh <= 16; sh += 8) {
        unsigned b = (bg >> sh) & 0xFF, f = (fg >> sh) & 0xFF;
        out |= ((b * (255 - a) + f * a + 127) / 255) << sh;
    }
    return out;
}

// Text in the toolkit's font (Inter SemiBold, anti-aliased) on a line 16
// pixels high whose top is y; what does not fit in maxw pixels ends in an
// ellipsis
static void text(int x, int y, const char *s, int maxw, uint32_t c, const rx_display_rect_t *r) {
    y += 12; // the baseline
    const titlefont_glyph_t *dots = title_glyph(0x2026);
    int room = maxw * 64 - (dots ? dots->adv : 0);
    int pen = 0; // 1/64 pixel
    while (*s) {
        const char *at = s;
        const titlefont_glyph_t *g = title_glyph(next_cp(&s));
        if (!g) continue;
        if (pen + g->adv > maxw * 64 || (*s && pen + g->adv > room)) {
            // Too long: an ellipsis instead of the rest (unless it all fits)
            int all = pen;
            for (const char *t = at; *t;) {
                const titlefont_glyph_t *h = title_glyph(next_cp(&t));
                all += h ? h->adv : 0;
            }
            if (all <= maxw * 64) goto draw;
            g = dots;
            if (!g || pen + g->adv > maxw * 64) break;
            s = "";
        }
    draw:;
        int gx = x + pen / 64 + g->dx, gy = y + g->dy;
        const uint8_t *cov = titlefont_data + g->off;
        for (int row = 0; row < g->h; row++) {
            int yy = gy + row;
            if (yy < (int)r->y || yy >= (int)(r->y + r->h)) continue;
            for (int col = 0; col < g->w; col++) {
                int xx = gx + col;
                unsigned a = cov[row * g->w + col];
                if (!a || xx < (int)r->x || xx >= (int)(r->x + r->w)) continue;
                uint32_t *px = &fbp[(uint64_t)yy * W + xx];
                *px = a == 255 ? c : blend(*px, c, a);
            }
        }
        pen += g->adv;
    }
}

// The title bar's buttons, right to left: close, maximise, minimise
static void button_rect(const window_t *w, int k, int *x, int *y) {
    *x = w->x + (int)w->w - (k + 1) * (BTN + 4);
    *y = w->y - TITLE_H + (TITLE_H - BTN) / 2;
}

static void draw_window(const window_t *w, int focused, const rx_display_rect_t *r) {
    if (!w->fullscreen && !w->panel && !w->desktop) {
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
        text(w->x + 10, w->y - TITLE_H + (TITLE_H - 16) / 2 + 1, w->title, room, focused ? C_TEXT : C_TEXT_OFF, r);
    }
    // The content
    int x0 = w->x > (int)r->x ? w->x : (int)r->x, y0 = w->y > (int)r->y ? w->y : (int)r->y;
    int x1 = w->x + (int)w->w < (int)(r->x + r->w) ? w->x + (int)w->w : (int)(r->x + r->w);
    int y1 = w->y + (int)w->h < (int)(r->y + r->h) ? w->y + (int)w->h : (int)(r->y + r->h);
    if (x1 <= x0) return;
    if (w->desktop) {
        // Premultiplied alpha in the top byte: over what is below
        for (int yy = y0; yy < y1; yy++) {
            uint32_t *dst = fbp + (uint64_t)yy * W + x0;
            const uint32_t *src = w->pix + (uint64_t)(yy - w->y) * w->w + (x0 - w->x);
            for (int i = 0; i < x1 - x0; i++) {
                uint32_t s = src[i], a = s >> 24;
                if (!a) continue;
                if (a == 255) {
                    dst[i] = s & 0xFFFFFF;
                    continue;
                }
                uint32_t d = dst[i], k = 255 - a;
                uint32_t rr = ((s >> 16) & 255) + (((d >> 16) & 255) * k + 127) / 255;
                uint32_t gg = ((s >> 8) & 255) + (((d >> 8) & 255) * k + 127) / 255;
                uint32_t bb = (s & 255) + ((d & 255) * k + 127) / 255;
                dst[i] = (rr > 255 ? 255 : rr) << 16 | (gg > 255 ? 255 : gg) << 8 | (bb > 255 ? 255 : bb);
            }
        }
        return;
    }
    for (int yy = y0; yy < y1; yy++)
        memcpy(fbp + (uint64_t)yy * W + x0, w->pix + (uint64_t)(yy - w->y) * w->w + (x0 - w->x), (size_t)(x1 - x0) * 4);
}

static void draw_cursor(const rx_display_rect_t *r) {
    if (hwcursor) return;
    const cursor_shape_t *c = &shapes[cur_shape];
    for (int row = 0; row < c->h; row++)
        for (int k = 0; c->rows[row][k]; k++) {
            int x = cx - c->hx + k, y = cy - c->hy + row;
            if (c->rows[row][k] == ' ' || x < 0 || y < 0 || x < (int)r->x || x >= (int)(r->x + r->w) || y < (int)r->y || y >= (int)(r->y + r->h))
                continue;
            fbp[(uint64_t)y * W + x] = c->rows[row][k] == '1' ? 0x000000 : 0xFFFFFF;
        }
}

static void damage_cursor(void) {
    const cursor_shape_t *c = &shapes[cur_shape];
    add_damage(cx - c->hx, cy - c->hy, c->w, c->h);
}

// ---- appearance ----

static char wall_choice[256] = "rustica"; // rustica, gradient, or a file of the user's
static uid_t session_uid = (uid_t)-1;

// A QOI image (qoiformat.org) as 0x00RRGGBB; its size in *w, *h. The file
// may be the user's: every length is checked.
static uint32_t *qoi_load(int fd, uint32_t *w, uint32_t *h) {
    struct stat st;
    if (fstat(fd, &st) < 0 || st.st_size < 22 || st.st_size > 64 << 20) return 0;
    size_t n = (size_t)st.st_size;
    uint8_t *b = malloc(n);
    if (!b) return 0;
    size_t got = 0;
    while (got < n) {
        ssize_t k = read(fd, b + got, n - got);
        if (k <= 0) break;
        got += (size_t)k;
    }
    uint32_t *px = 0;
    if (got == n && !memcmp(b, "qoif", 4)) {
        uint32_t iw = (uint32_t)b[4] << 24 | (uint32_t)b[5] << 16 | (uint32_t)b[6] << 8 | b[7];
        uint32_t ih = (uint32_t)b[8] << 24 | (uint32_t)b[9] << 16 | (uint32_t)b[10] << 8 | b[11];
        if (iw && ih && iw <= 16384 && ih <= 16384 && (uint64_t)iw * ih <= 40u << 20)
            px = malloc((size_t)iw * ih * 4);
        if (px) {
            uint8_t idx[64][4];
            memset(idx, 0, sizeof idx);
            uint8_t c[4] = { 0, 0, 0, 255 };
            size_t p = 14, end = n - 8, total = (size_t)iw * ih;
            int run = 0;
            for (size_t i = 0; i < total; i++) {
                if (run > 0) {
                    run--;
                } else if (p < end) {
                    uint8_t b1 = b[p++];
                    if (b1 == 0xFE && p + 3 <= end) {
                        c[0] = b[p]; c[1] = b[p + 1]; c[2] = b[p + 2]; p += 3;
                    } else if (b1 == 0xFF && p + 4 <= end) {
                        c[0] = b[p]; c[1] = b[p + 1]; c[2] = b[p + 2]; c[3] = b[p + 3]; p += 4;
                    } else if ((b1 & 0xC0) == 0x00) {
                        memcpy(c, idx[b1], 4);
                    } else if ((b1 & 0xC0) == 0x40) {
                        c[0] += ((b1 >> 4) & 3) - 2; c[1] += ((b1 >> 2) & 3) - 2; c[2] += (b1 & 3) - 2;
                    } else if ((b1 & 0xC0) == 0x80 && p < end) {
                        uint8_t b2 = b[p++];
                        int vg = (b1 & 0x3F) - 32;
                        c[0] += vg - 8 + ((b2 >> 4) & 0xF); c[1] += vg; c[2] += vg - 8 + (b2 & 0xF);
                    } else if ((b1 & 0xC0) == 0xC0) {
                        run = b1 & 0x3F;
                    }
                    memcpy(idx[(c[0] * 3 + c[1] * 5 + c[2] * 7 + c[3] * 11) % 64], c, 4);
                }
                px[i] = (uint32_t)c[0] << 16 | (uint32_t)c[1] << 8 | c[2];
            }
            *w = iw;
            *h = ih;
        }
    }
    free(b);
    return px;
}

// The wallpaper, computed once for the screen's size
static uint32_t *wallpaper;

// A gradient in the theme's colours with a soft glow of its accent low on
// the left
static void make_gradient(void) {
    // Ordered dithering (4x4 Bayer) keeps the gradient free of bands
    static const int bayer[4][4] = { { 0, 8, 2, 10 }, { 12, 4, 14, 6 }, { 3, 11, 1, 9 }, { 15, 7, 13, 5 } };
    int64_t rad = (int64_t)H * 10 / 14, r2 = rad * rad;
    uint32_t top = th->desktop, bot = th->surface, acc = th->border;
    int64_t t0r = top >> 16 & 255, t0g = top >> 8 & 255, t0b = top & 255;
    int64_t t1r = bot >> 16 & 255, t1g = bot >> 8 & 255, t1b = bot & 255;
    int64_t ar = acc >> 16 & 255, ag = acc >> 8 & 255, ab = acc & 255;
    for (uint32_t y = 0; y < H; y++) {
        int64_t t = (int64_t)y * 256 / H;
        int64_t br = t0r * 256 + (t1r - t0r) * t, bg = t0g * 256 + (t1g - t0g) * t, bb = t0b * 256 + (t1b - t0b) * t;
        for (uint32_t x = 0; x < W; x++) {
            int64_t dx = (int64_t)x - W / 4, dy = (int64_t)y - 3 * H / 4, d2 = dx * dx + dy * dy;
            int64_t f = d2 < r2 ? (r2 - d2) * 4096 / r2 : 0;
            int64_t g = 41 * f * f / 4096;
            int64_t r = br + ((ar * 256 - br) * g) / (256 * 4096), gg = bg + ((ag * 256 - bg) * g) / (256 * 4096),
                    b = bb + ((ab * 256 - bb) * g) / (256 * 4096);
            int dth = bayer[y & 3][x & 3] * 16 + 8;
            wallpaper[(uint64_t)y * W + x] = (uint32_t)((r + dth) >> 8) << 16 | (uint32_t)((gg + dth) >> 8) << 8 |
                                             (uint32_t)((b + dth) >> 8);
        }
    }
}

// An image scaled to cover the screen (cut at the sides that stick out),
// bilinear
static void cover(const uint32_t *src, uint32_t sw, uint32_t sh) {
    // Scale in 1/65536ths: the source pixels per screen pixel
    uint64_t sx = (uint64_t)sw * 65536 / W, sy = (uint64_t)sh * 65536 / H;
    uint64_t s = sx < sy ? sx : sy;
    uint64_t ox = ((uint64_t)sw * 65536 - s * W) / 2, oy = ((uint64_t)sh * 65536 - s * H) / 2;
    for (uint32_t y = 0; y < H; y++) {
        uint64_t fy = oy + y * s + s / 2;
        fy = fy > 32768 ? fy - 32768 : 0;
        uint32_t y0 = (uint32_t)(fy >> 16), y1 = y0 + 1 < sh ? y0 + 1 : y0;
        uint32_t wy = (uint32_t)(fy & 0xFFFF) >> 8;
        if (y0 >= sh) y0 = y1 = sh - 1;
        for (uint32_t x = 0; x < W; x++) {
            uint64_t fx = ox + x * s + s / 2;
            fx = fx > 32768 ? fx - 32768 : 0;
            uint32_t x0 = (uint32_t)(fx >> 16), x1 = x0 + 1 < sw ? x0 + 1 : x0;
            uint32_t wx = (uint32_t)(fx & 0xFFFF) >> 8;
            if (x0 >= sw) x0 = x1 = sw - 1;
            uint32_t a = src[(uint64_t)y0 * sw + x0], b = src[(uint64_t)y0 * sw + x1];
            uint32_t c = src[(uint64_t)y1 * sw + x0], d = src[(uint64_t)y1 * sw + x1];
            uint32_t out = 0;
            for (int sh8 = 0; sh8 <= 16; sh8 += 8) {
                uint32_t top = ((a >> sh8 & 255) * (256 - wx) + (b >> sh8 & 255) * wx) >> 8;
                uint32_t bot = ((c >> sh8 & 255) * (256 - wx) + (d >> sh8 & 255) * wx) >> 8;
                out |= ((top * (256 - wy) + bot * wy) >> 8) << sh8;
            }
            wallpaper[(uint64_t)y * W + x] = out;
        }
    }
}

// Open a file of the session user's (theirs and a plain file: as root we
// must not show them another user's file, or a system one)
static int open_users(const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) return -1;
    struct stat st;
    if (fstat(fd, &st) < 0 || !S_ISREG(st.st_mode) || st.st_uid != session_uid) {
        close(fd);
        return -1;
    }
    return fd;
}

static void make_wallpaper(void) {
    free(wallpaper);
    wallpaper = malloc((size_t)W * H * 4);
    if (!wallpaper) return;
    if (strcmp(wall_choice, "gradient")) {
        int fd;
        if (wall_choice[0] == '/') {
            fd = open_users(wall_choice);
        } else if (theme_name_ok(wall_choice) && strcmp(wall_choice, "rustica")) {
            // The system's others (2026-11-25): dracula, forest, arctic, sakura
            char p[96];
            snprintf(p, sizeof p, "/usr/share/backgrounds/%s.qoi", wall_choice);
            fd = open(p, O_RDONLY);
        } else {
            fd = open(H > W ? "/usr/share/backgrounds/rustica-fox-portrait.qoi" : "/usr/share/backgrounds/rustica-fox.qoi",
                      O_RDONLY);
        }
        if (fd >= 0) {
            uint32_t sw, sh, *src = qoi_load(fd, &sw, &sh);
            close(fd);
            if (src) {
                cover(src, sw, sh);
                free(src);
                vlog("wallpaper %s (%ux%u)\n", wall_choice, sw, sh);
                return;
            }
        }
        vlog("wallpaper %s: cannot read it, the gradient instead\n", wall_choice);
    }
    make_gradient();
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
    int x = ((int)PW - LOCK_W) / 2, y = ((int)PH - LOCK_H) / 2;
    fill(x, y, LOCK_W, LOCK_H, th->surface, r);
    fill(x, y, LOCK_W, 2, C_BORDER, r);
    char line[96];
    snprintf(line, sizeof line, "Locked: %s", session_user);
    text(x + 24, y + 22, line, LOCK_W - 48, C_TEXT, r);
    text(x + 24, y + 48, "Type your password and Enter", LOCK_W - 48, C_TEXT_OFF, r);
    fill(x + 24, y + 78, LOCK_W - 48, 28, th->field, r);
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
    int ax, ay, aw, ah;
    work_area(&ax, &ay, &aw, &ah);
    *x = ax + aw - TOAST_W - 16;
    *y = ay + 16 + i * (TOAST_H + 8);
}

static void draw_toasts(const rx_display_rect_t *r) {
    for (int i = 0; i < ntoasts; i++) {
        int x, y;
        toast_rect(i, &x, &y);
        fill(x, y, TOAST_W, TOAST_H, th->surface, r);
        fill(x, y, 3, TOAST_H, C_BORDER, r);
        text(x + 14, y + 16, toasts[i].text, TOAST_W - 24, C_TEXT, r);
    }
}

static void draw_menu(const rx_display_rect_t *r);

// Drag and drop between windows (step F12): what is dragged (paths, a
// line each), its label by the pointer, the window it came from
// Power settings (Settings > Power, step F15): blank the screen after so
// many idle minutes (0 never), with what (blank: black; dim: the
// wallpaper, dimmed), and lock it then
static int idle_minutes = 15, idle_lock;
static char saver_kind[16] = "blank";
static uint64_t last_input;
static int saver_on;

static int grab_win = -1;       // the window a button went down in
static int dnd_active, dnd_src = -1;
static char dnd_text[RX_WINDOW_MAX_CLIPBOARD + 1];
static size_t dnd_len;
static char dnd_label[64];
#define DND_W 220
#define DND_H 26

static void damage_dnd(void) {
    if (dnd_active) add_damage(cx + 14, cy + 14, DND_W, DND_H);
}

static void draw_dnd(const rx_display_rect_t *r) {
    if (!dnd_active) return;
    int x = cx + 14, y = cy + 14;
    fill(x, y, DND_W, DND_H, C_BORDER, r);
    fill(x + 1, y + 1, DND_W - 2, DND_H - 2, th->surface, r);
    text(x + 10, y + (DND_H - 16) / 2, dnd_label, DND_W - 20, C_TEXT, r);
}

// The tiling preview while a window is dragged to an edge
static int snap_zone;
static void draw_snap(const rx_display_rect_t *r) {
    if (!snap_zone) return;
    int x, y, w, h;
    snap_rect(snap_zone, &x, &y, &w, &h);
    x += 6; y += 6; w -= 12; h -= 12;
    // A tint of the accent over what is there, and its outline
    int x0 = x > (int)r->x ? x : (int)r->x, y0 = y > (int)r->y ? y : (int)r->y;
    int x1 = x + w < (int)(r->x + r->w) ? x + w : (int)(r->x + r->w);
    int y1 = y + h < (int)(r->y + r->h) ? y + h : (int)(r->y + r->h);
    for (int py = y0; py < y1; py++) {
        uint32_t *row = fbp + (uint64_t)py * W;
        for (int px = x0; px < x1; px++) row[px] = blend(row[px], C_BORDER, 60);
    }
    fill(x, y, w, 2, C_BORDER, r);
    fill(x, y + h - 2, w, 2, C_BORDER, r);
    fill(x, y, 2, h, C_BORDER, r);
    fill(x + w - 2, y, 2, h, C_BORDER, r);
}

static void damage_snap(int z) {
    if (!z) return;
    int x, y, w, h;
    snap_rect(z, &x, &y, &w, &h);
    add_damage(x, y, w, h);
}

static void compose(void) {
    if (!ndamage) return;
    for (int i = 0; i < ndamage; i++) {
        const rx_display_rect_t *r = &damage[i];
        if (saver_on) {
            // The screensaver: black, or the wallpaper at a quarter
            if (!strcmp(saver_kind, "dim")) {
                draw_wallpaper(r);
                for (uint32_t yy = r->y; yy < r->y + r->h; yy++)
                    for (uint32_t xx = r->x; xx < r->x + r->w; xx++) {
                        uint32_t *p = &fbp[(uint64_t)yy * W + xx];
                        *p = (*p >> 2) & 0x3F3F3F;
                    }
            } else {
                fill((int)r->x, (int)r->y, (int)r->w, (int)r->h, 0x000000, r);
            }
            continue;
        }
        draw_wallpaper(r);
        if (locked) {
            draw_lock(r);
        } else {
            for (int k = 0; k < norder; k++) {
                const window_t *w = &wins[order[k]];
                if (w->minimized || !w->pix || (w->panel && !panel_shown)) continue;
                draw_window(w, order[k] == focus, r);
            }
            draw_snap(r);
            draw_toasts(r);
            draw_dnd(r);
            draw_menu(r);
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

// The theme's name and colours (the toolkit draws in them)
static void send_appearance(window_t *w) {
    static char out[sizeof(rx_window_event_t) + sizeof theme_blob];
    rx_window_event_t ev;
    memset(&ev, 0, sizeof ev);
    ev.op = RX_WINDOW_EV_APPEARANCE;
    snprintf(ev.text, sizeof ev.text, "%s", th->name);
    ev.n = (uint32_t)theme_blob_len;
    ev.time = rx_clock_get_monotonic();
    memcpy(out, &ev, sizeof ev);
    memcpy(out + sizeof ev, theme_blob, (size_t)theme_blob_len);
    rx_channel_write(w->ch, 0, out, (uint32_t)(sizeof ev + (size_t)theme_blob_len), 0, 0);
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
            if (w->panel || w->desktop) continue;
            memset(&ev, 0, sizeof ev);
            ev.op = RX_WINDOW_EV_WINDOW_INFO;
            ev.x = order[i];
            ev.flags = (order[i] == focus ? 1u : 0u) | (w->minimized ? 2u : 0u);
            snprintf(ev.text, sizeof ev.text, "%s", w->title);
            // The app, a 0 byte, the whole title (as much as fits)
            size_t al = strlen(w->app);
            memcpy(ev.info, w->app, al);
            snprintf(ev.info + al + 1, sizeof ev.info - al - 1, "%s", w->title);
            send_event(&wins[p], &ev);
        }
    }
}

// key=value lines of one file into the theme's and wallpaper's names
static void read_appearance(int fd, char *theme, size_t tn, char *wall, size_t wn) {
    char buf[1024];
    ssize_t n = read(fd, buf, sizeof buf - 1);
    if (n <= 0) return;
    buf[n] = 0;
    for (char *save, *l = strtok_r(buf, "\n", &save); l; l = strtok_r(0, "\n", &save)) {
        while (*l == ' ' || *l == '\t') l++;
        char *v = strchr(l, '=');
        if (!v) continue;
        *v++ = 0;
        v[strcspn(v, " \t\r")] = 0;
        if (!strcmp(l, "theme")) snprintf(theme, tn, "%s", v);
        if (!strcmp(l, "wallpaper")) snprintf(wall, wn, "%s", v);
        if (!strcmp(l, "blank")) idle_minutes = atoi(v) < 0 ? 0 : atoi(v);
        if (!strcmp(l, "screensaver")) snprintf(saver_kind, sizeof saver_kind, "%s", v);
        if (!strcmp(l, "lock_on_blank")) idle_lock = !strcmp(v, "yes") || !strcmp(v, "1") || !strcmp(v, "true");
    }
}

// The appearance now: the system's, then the session user's; on a change
// the wallpaper is made again, everything redrawn and every window told
static char conf_path[160];
static struct timespec conf_mtime;
static off_t conf_size = -1;

static void load_appearance(int force) {
    struct stat st;
    int have = conf_path[0] && stat(conf_path, &st) == 0;
    if (!force) {
        if (!have && conf_size == -1) return;
        if (have && st.st_mtim.tv_sec == conf_mtime.tv_sec && st.st_mtim.tv_nsec == conf_mtime.tv_nsec &&
            st.st_size == conf_size)
            return;
    }
    if (have) {
        conf_mtime = st.st_mtim;
        conf_size = st.st_size;
    } else {
        conf_size = -1;
    }
    char theme[32] = "rustica", wall[256] = "rustica";
    idle_minutes = 15;
    idle_lock = 0;
    snprintf(saver_kind, sizeof saver_kind, "blank");
    int fd = open("/etc/rustica/appearance", O_RDONLY);
    if (fd >= 0) {
        read_appearance(fd, theme, sizeof theme, wall, sizeof wall);
        close(fd);
    }
    if (conf_path[0] && (fd = open_users(conf_path)) >= 0) {
        read_appearance(fd, theme, sizeof theme, wall, sizeof wall);
        close(fd);
    }
    // The theme: the user's own file of that name, the system's, else Rustica
    theme_t nt = theme_builtin;
    int found = 0;
    if (theme_name_ok(theme) && conf_path[0]) {
        char p[300];
        snprintf(p, sizeof p, "%s", conf_path);
        char *slash = strrchr(p, '/');
        if (slash) {
            snprintf(slash + 1, sizeof p - (size_t)(slash + 1 - p), "rustica/themes/%s.theme", theme);
            found = theme_read_fd(&nt, theme, open_users(p)) == 0;
        }
    }
    if (!found) found = theme_load_system(&nt, theme) == 0;
    if (!found) nt = theme_builtin;
    char blob[sizeof theme_blob];
    int bl = theme_text(&nt, blob, sizeof blob);
    int changed = strcmp(nt.name, cur_theme.name) || bl != theme_blob_len || memcmp(blob, theme_blob, (size_t)bl) ||
                  strcmp(wall, wall_choice) || force;
    if (!changed) return;
    int wall_changed = strcmp(wall, wall_choice) || force || !wallpaper;
    use_theme(&nt);
    snprintf(wall_choice, sizeof wall_choice, "%s", wall);
    vlog("appearance: theme %s%s, wallpaper %s\n", th->name, found ? "" : " (built in)", wall_choice);
    if (wall_changed || !strcmp(wall_choice, "gradient")) make_wallpaper();
    add_damage(0, 0, (int)W, (int)H);
    for (int k = 0; k < MAX_WINDOWS; k++)
        if (wins[k].used) send_appearance(&wins[k]);
}

// Whose appearance: the session user's (or none)
static void set_session_user(const char *name) {
    snprintf(session_user, sizeof session_user, "%s", name);
    conf_path[0] = 0;
    session_uid = (uid_t)-1;
    struct passwd *pw = name[0] ? getpwnam(name) : 0;
    if (pw) {
        session_uid = pw->pw_uid;
        snprintf(conf_path, sizeof conf_path, "%s/.config/rustica.conf", pw->pw_dir);
    }
    load_appearance(1);
}

// ---- the desktop's menu (a right click on the wallpaper) ----

#define MENU_W 220
#define MENU_ROW 28
static const char *menu_items[] = { "Change wallpaper...", "Appearance settings..." };
#define MENU_N 2
static int menu_open, menu_x, menu_y, menu_hover = -1;

static void draw_menu(const rx_display_rect_t *r) {
    if (!menu_open) return;
    int h = MENU_N * MENU_ROW + 8;
    fill(menu_x, menu_y, MENU_W, h, C_BORDER_OFF, r);
    fill(menu_x + 1, menu_y + 1, MENU_W - 2, h - 2, th->surface, r);
    for (int i = 0; i < MENU_N; i++) {
        int y = menu_y + 4 + i * MENU_ROW;
        if (i == menu_hover) {
            fill(menu_x + 4, y, MENU_W - 8, MENU_ROW, th->hover, r);
            fill(menu_x + 4, y, 3, MENU_ROW, C_BORDER, r);
        }
        text(menu_x + 16, y + (MENU_ROW - 16) / 2, menu_items[i], MENU_W - 28, C_TEXT, r);
    }
}

static void damage_menu(void) {
    add_damage(menu_x, menu_y, MENU_W, MENU_N * MENU_ROW + 8);
}

static int menu_item_at(int x, int y) {
    if (x < menu_x + 4 || x >= menu_x + MENU_W - 4 || y < menu_y + 4) return -1;
    int i = (y - menu_y - 4) / MENU_ROW;
    return i < MENU_N ? i : -1;
}

// What the menu's choice does is the panel's to do (it runs as the user)
static void menu_choose(int i) {
    vlog("desktop menu: %s\n", menu_items[i]);
    for (int k = 0; k < MAX_WINDOWS; k++)
        if (wins[k].used && wins[k].panel) send_simple(&wins[k], RX_WINDOW_EV_DESKTOP, (uint32_t)i + 1);
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
    // The desktop stays at the bottom
    for (int j = 1; j < norder; j++) {
        int p = order[j];
        if (!wins[p].desktop) continue;
        for (int m = j; m > 0; m--) order[m] = order[m - 1];
        order[0] = p;
    }
}

// The topmost window that is not minimised (nor a panel or the desktop), or -1
static int top_window(void) {
    for (int i = norder - 1; i >= 0; i--)
        if (!wins[order[i]].minimized && !wins[order[i]].panel && !wins[order[i]].desktop) return order[i];
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
    if (dnd_active && dnd_src == k) {
        damage_dnd();
        dnd_active = 0;
        dnd_src = -1;
    }
    if (grab_win == k) grab_win = -1;
    int was_panel = w->panel;
    memset(w, 0, sizeof *w);
    if (was_panel) {
        panel_size = 0;
        relayout();
    }
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

// Put a window's frame in a part of the work area
static void place_in(window_t *w, int z) {
    int x, y, fw, fh;
    snap_rect(z, &x, &y, &fw, &fh);
    damage_window(w);
    w->x = x + BORDER;
    w->y = y + TITLE_H + BORDER;
    configure(w, (uint32_t)(fw - 2 * BORDER), (uint32_t)(fh - TITLE_H - 2 * BORDER));
    damage_window(w);
}

// Tile a window (SNAP_LEFT ... SNAP_BR), or untile it (SNAP_NONE)
static void tile(int k, int z) {
    window_t *w = &wins[k];
    if (w->fullscreen || w->panel || w->desktop) return;
    if (z == SNAP_MAX) {
        if (!w->maximized) toggle_maximize(k);
        return;
    }
    if (z == SNAP_NONE) {
        if (w->maximized) toggle_maximize(k);
        else if (w->tiled) {
            w->tiled = 0;
            restore_geometry(w);
            vlog("window \"%s\" untiled\n", w->title);
        }
        return;
    }
    if (!w->maximized && !w->tiled) save_geometry(w);
    w->maximized = 0;
    w->tiled = z;
    place_in(w, z);
    static const char *names[] = { "", "left", "right", "top left", "top right", "bottom left", "bottom right" };
    vlog("window \"%s\" tiled %s\n", w->title, names[z]);
    send_state(w);
}

// The panel at its edge
static void place_panel(window_t *w) {
    damage_window(w);
    switch (panel_edge) {
    case RX_WINDOW_EDGE_TOP: w->x = 0; w->y = 0; configure(w, PW, (uint32_t)panel_size); break;
    case RX_WINDOW_EDGE_LEFT: w->x = 0; w->y = 0; configure(w, (uint32_t)panel_size, PH); break;
    case RX_WINDOW_EDGE_RIGHT: w->x = (int)PW - panel_size; w->y = 0; configure(w, (uint32_t)panel_size, PH); break;
    default: w->x = 0; w->y = (int)PH - panel_size; configure(w, PW, (uint32_t)panel_size); break;
    }
    damage_window(w);
}

// The work area changed (the panel moved, the screen resized): maximised
// and tiled windows follow, the others keep their title bars reachable
static void relayout(void) {
    int ax, ay, aw, ah;
    work_area(&ax, &ay, &aw, &ah);
    for (int k = 0; k < MAX_WINDOWS; k++) {
        window_t *w = &wins[k];
        if (!w->used) continue;
        if (w->panel) {
            place_panel(w);
        } else if (w->desktop) {
            damage_window(w);
            w->x = ax;
            w->y = ay;
            configure(w, (uint32_t)aw, (uint32_t)ah);
            damage_window(w);
        } else if (w->fullscreen) {
            configure(w, PW, PH);
        } else if (w->maximized) {
            place_in(w, SNAP_MAX);
        } else if (w->tiled) {
            place_in(w, w->tiled);
        } else {
            damage_window(w);
            if (w->x > ax + aw - 64) w->x = ax + aw - 64 > ax + BORDER ? ax + aw - 64 : ax + BORDER;
            if (w->x + (int)w->w < ax + 64) w->x = ax + 64 - (int)w->w;
            if (w->y > ay + ah - 8) w->y = ay + ah - 8 > ay + TITLE_H + BORDER ? ay + ah - 8 : ay + TITLE_H + BORDER;
            if (w->y < ay + TITLE_H + BORDER) w->y = ay + TITLE_H + BORDER;
            damage_window(w);
        }
    }
}

static void toggle_maximize(int k) {
    window_t *w = &wins[k];
    if (w->fullscreen || w->desktop || w->panel) return;
    if (w->maximized) {
        w->maximized = 0;
        restore_geometry(w);
    } else {
        if (!w->tiled) save_geometry(w);
        w->tiled = 0;
        w->maximized = 1;
        place_in(w, SNAP_MAX);
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
        configure(w, PW, PH);
    } else {
        w->fullscreen = 0;
        if (w->maximized) {
            place_in(w, SNAP_MAX);
        } else if (w->tiled) {
            place_in(w, w->tiled);
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
    } else if (r->x != -1 || r->y != -1) {
        // RX_WINDOW_AT_END (-2) for x or y: against the work area's right
        // or bottom edge (a menu by the panel), the other as given
        int ax, ay, aw, ah;
        work_area(&ax, &ay, &aw, &ah);
        w->x = r->x == RX_WINDOW_AT_END ? ax + aw - (int)r->w - BORDER - 8 : r->x;
        w->y = r->y == RX_WINDOW_AT_END ? ay + ah - (int)r->h - BORDER - 8 : r->y;
        if (w->x < ax + BORDER) w->x = ax + BORDER;
        if (w->y < ay + TITLE_H + BORDER) w->y = ay + TITLE_H + BORDER;
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
    add_damage((int)PW - TOAST_W - 16, 16, TOAST_W, MAX_TOASTS * (TOAST_H + 8));
    vlog("notification \"%s\"\n", toasts[ntoasts - 1].text);
}

static void expire_toasts(uint64_t now) {
    int j = 0;
    for (int i = 0; i < ntoasts; i++)
        if (toasts[i].until > now) toasts[j++] = toasts[i];
    if (j != ntoasts) {
        int x, y;
        toast_rect(0, &x, &y);
        add_damage(x, y, TOAST_W, MAX_TOASTS * (TOAST_H + 8));
    }
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
    add_damage(((int)PW - LOCK_W) / 2, ((int)PH - LOCK_H) / 2, LOCK_W, LOCK_H);
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
        case RX_WINDOW_DRAG_START:
            // Only while the left button is held in this window
            if (grab_win == k && (buttons & 1) && plen > 0) {
                dnd_len = plen < RX_WINDOW_MAX_CLIPBOARD ? plen : RX_WINDOW_MAX_CLIPBOARD;
                memcpy(dnd_text, payload, dnd_len);
                dnd_text[dnd_len] = 0;
                // The label: the first name, and how many more
                const char *first = dnd_text, *nl = strchr(dnd_text, '\n');
                size_t fl = nl ? (size_t)(nl - dnd_text) : dnd_len;
                const char *slash = first;
                for (size_t i = 0; i < fl; i++)
                    if (first[i] == '/') slash = first + i + 1;
                int more = 0;
                for (size_t i = 0; i < dnd_len; i++)
                    if (dnd_text[i] == '\n' && i + 1 < dnd_len) more++;
                size_t nl2 = fl - (size_t)(slash - first);
                if (nl2 > 40) nl2 = 40;
                if (more)
                    snprintf(dnd_label, sizeof dnd_label, "%.*s and %d more", (int)nl2, slash, more);
                else
                    snprintf(dnd_label, sizeof dnd_label, "%.*s", (int)nl2, slash);
                dnd_active = 1;
                dnd_src = k;
                damage_dnd();
                vlog("drag from \"%s\": %s\n", w->title, dnd_label);
            }
            break;
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
        case RX_WINDOW_SET_CURSOR:
            w->cursor = r.flags < RX_WINDOW_CURSOR_COUNT ? r.flags : RX_WINDOW_CURSOR_ARROW;
            break;
        case RX_WINDOW_SET_APP: {
            // Letters, digits, '.', '-', '_' only: a name the panel looks up
            size_t al = plen < sizeof w->app - 1 ? plen : sizeof w->app - 1;
            int ok = al > 0;
            for (size_t i = 0; i < al; i++) {
                char c = payload[i];
                if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '.' || c == '-' || c == '_')) ok = 0;
            }
            if (ok) {
                memcpy(w->app, payload, al);
                w->app[al] = 0;
                notify_panels();
            }
            break;
        }
        case RX_WINDOW_SET_FULLSCREEN:
            if (!w->panel) set_fullscreen(k, r.flags != 0);
            break;
        case RX_WINDOW_SET_ROLE:
            if (r.flags == RX_WINDOW_ROLE_DESKTOP && !w->panel && !w->desktop) {
                // The desktop's icons: the work area, below every window
                damage_window(w);
                w->desktop = 1;
                int ax, ay, aw, ah;
                work_area(&ax, &ay, &aw, &ah);
                w->x = ax;
                w->y = ay;
                configure(w, (uint32_t)aw, (uint32_t)ah);
                raise(k); // which puts it at the bottom
                damage_window(w);
                vlog("window \"%s\" is the desktop\n", w->title);
                notify_panels();
            }
            if (r.flags == RX_WINDOW_ROLE_PANEL && !w->panel) {
                // Docked at the bottom, the screen's width, on top
                damage_window(w);
                if (focus == k) {
                    focus = -1;
                    set_focus(top_window());
                }
                w->panel = 1;
                panel_edge = RX_WINDOW_EDGE_BOTTOM;
                panel_size = (int)w->h;
                panel_autohide = 0;
                panel_shown = 1;
                place_panel(w);
                raise(k);
                relayout();
                damage_window(w);
                vlog("window \"%s\" is the panel\n", w->title);
                notify_panels();
            }
            break;
        case RX_WINDOW_SET_PANEL:
            // The panel's edge (flags), thickness (x) and auto-hide (y)
            if (w->panel && r.flags <= RX_WINDOW_EDGE_RIGHT && r.x >= 24 && r.x <= 400) {
                damage_window(w);
                panel_edge = (int)r.flags;
                panel_size = r.x;
                panel_autohide = r.y != 0;
                panel_shown = !panel_autohide;
                static const char *edges[] = { "bottom", "top", "left", "right" };
                vlog("panel: %s, %d, %s\n", edges[panel_edge], panel_size, panel_autohide ? "auto-hide" : "always shown");
                relayout();
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
    // Its first event: the theme to draw in (a sandboxed app cannot read
    // the user's own theme files)
    send_appearance(&wins[k]);
    set_focus(k);
    return 1;
}

// ---- input ----

enum { DRAG_NONE, DRAG_MOVE, DRAG_RESIZE };
static int drag, drag_win, drag_dx, drag_dy, drag_edges;  // edges: 1 right, 2 bottom
static int pointer_in = -1;     // the window that last got a pointer event
static uint64_t last_click;
static int last_click_win = -1;

enum { HIT_NONE, HIT_CONTENT, HIT_TITLE, HIT_CLOSE, HIT_MAX, HIT_MIN, HIT_RESIZE };

static int hit(int px, int py, int *which, int *edges) {
    for (int i = norder - 1; i >= 0; i--) {
        int k = order[i];
        const window_t *w = &wins[k];
        if (w->minimized || (w->panel && !panel_shown)) continue;
        *which = k;
        if (px >= w->x && px < w->x + (int)w->w && py >= w->y && py < w->y + (int)w->h) return HIT_CONTENT;
        if (w->fullscreen || w->panel || w->desktop) continue;
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

// The shape for a resize zone (edges: 1 right, 2 bottom)
static uint32_t resize_shape(int edges) {
    return edges == 1 ? RX_WINDOW_CURSOR_RESIZE_H : edges == 2 ? RX_WINDOW_CURSOR_RESIZE_V : RX_WINDOW_CURSOR_RESIZE_NWSE;
}

// The pointer's shape for where it is: the window's own over its content,
// the resize arrows on an edge (and while resizing), else the arrow
static void update_cursor_shape(void) {
    uint32_t s = RX_WINDOW_CURSOR_ARROW;
    if (locked || saver_on) {
        s = RX_WINDOW_CURSOR_ARROW;
    } else if (drag == DRAG_RESIZE) {
        s = resize_shape(drag_edges);
    } else if (drag == DRAG_NONE) {
        int k = -1, e = 0;
        int h = hit(cx, cy, &k, &e);
        if (h == HIT_CONTENT && k >= 0) s = wins[k].cursor;
        else if (h == HIT_RESIZE) s = resize_shape(e);
    }
    if (s >= RX_WINDOW_CURSOR_COUNT) s = RX_WINDOW_CURSOR_ARROW;
    if (s == cur_shape) return;
    damage_cursor();
    cur_shape = s;
    damage_cursor();
    if (hwcursor) upload_hwcursor();
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
    if (hwcursor) {
        cx = nx;
        cy = ny;
        rx_display_cursor_move(&d, cx, cy, 1);
        return;
    }
    damage_cursor();
    cx = nx;
    cy = ny;
    damage_cursor();
}

// The current shape as a 64 x 64 image for the display's hardware cursor
static int upload_hwcursor(void) {
    static uint32_t img[64 * 64];
    const cursor_shape_t *c = &shapes[cur_shape];
    memset(img, 0, sizeof img);
    for (int row = 0; row < c->h && row < 64; row++)
        for (int k = 0; c->rows[row][k] && k < 64; k++)
            if (c->rows[row][k] != ' ') img[row * 64 + k] = c->rows[row][k] == '1' ? 0xFF000000u : 0xFFFFFFFFu;
    return rx_display_cursor_set(&d, img, (uint32_t)c->hx, (uint32_t)c->hy);
}

// The pointer as the display's hardware cursor, if it has one
static void setup_hwcursor(void) {
    if (!(d.caps & RX_DISPLAY_CAP_CURSOR)) return;
    if (upload_hwcursor() == 0) {
        hwcursor = 1;
        rx_display_cursor_move(&d, cx, cy, 1);
        vlog("hardware cursor\n");
    }
}

// The display changed size (step F8): everything follows
static void screen_resized(void) {
    W = d.width;
    H = d.height;
    rx_display_primary(&d, &PW, &PH);
    fbp = d.pixels;
    make_wallpaper();
    if (cx >= (int)W) cx = (int)W - 1;
    if (cy >= (int)H) cy = (int)H - 1;
    relayout();
    if (hwcursor) rx_display_cursor_move(&d, cx, cy, 1);
    vlog("the screen is %ux%u (the first head %ux%u)\n", W, H, PW, PH);
    add_damage(0, 0, (int)W, (int)H);
}

static void pointer_event(const rx_input_event_t *e) {
    if (locked) {
        // The cursor moves; nothing else
        if (e->flags & RX_INPUT_ABSOLUTE)
            move_cursor((int)((uint64_t)e->x * (PW - 1) / 32767), (int)((uint64_t)e->y * (PH - 1) / 32767));
        else
            move_cursor(cx + e->x, cy + e->y);
        buttons = e->code;
        return;
    }
    int ox = cx, oy = cy;
    if (e->flags & RX_INPUT_ABSOLUTE)
        move_cursor((int)((uint64_t)e->x * (PW - 1) / 32767), (int)((uint64_t)e->y * (PH - 1) / 32767));
    else
        move_cursor(cx + e->x, cy + e->y);
    int moved = cx != ox || cy != oy;
    if (panel_autohide && panel_size && moved && drag == DRAG_NONE) {
        int near;
        switch (panel_edge) {
        case RX_WINDOW_EDGE_TOP: near = cy <= (panel_shown ? panel_size : 1); break;
        case RX_WINDOW_EDGE_LEFT: near = cx <= (panel_shown ? panel_size : 1); break;
        case RX_WINDOW_EDGE_RIGHT: near = cx >= (int)PW - (panel_shown ? panel_size : 2); break;
        default: near = cy >= (int)PH - (panel_shown ? panel_size : 2); break;
        }
        if (near != panel_shown) {
            panel_shown = near;
            for (int k = 0; k < MAX_WINDOWS; k++)
                if (wins[k].used && wins[k].panel) {
                    damage_window(&wins[k]);
                    if (near) raise(k);
                }
        }
    }
    uint32_t old = buttons;
    buttons = e->code;
    uint32_t pressed = buttons & ~old, released = old & ~buttons;

    // A drag in progress
    if (drag != DRAG_NONE) {
        window_t *w = &wins[drag_win];
        if (moved && drag == DRAG_MOVE) {
            if (w->maximized || w->tiled) {
                // Dragged out of its tile: its own size again, under the pointer
                int fw = (int)w->sw;
                if (fw <= 0) fw = (int)w->w;
                drag_dx = drag_dx * fw / ((int)w->w > 0 ? (int)w->w : 1);
                w->maximized = 0;
                w->tiled = 0;
                damage_window(w);
                configure(w, w->sw ? w->sw : w->w, w->sh ? w->sh : w->h);
                send_state(w);
            }
            damage_window(w);
            w->x = cx - drag_dx;
            w->y = cy - drag_dy;
            if (w->y < TITLE_H + BORDER) w->y = TITLE_H + BORDER; // the title bar stays reachable
            damage_window(w);
            // Near an edge of the work area: where it would snap
            int ax, ay, aw, ah, z = SNAP_NONE;
            work_area(&ax, &ay, &aw, &ah);
            int left = cx <= ax + 2, right = cx >= ax + aw - 3, top = cy <= ay + 2;
            int upper = cy < ay + ah / 4, lower = cy > ay + ah * 3 / 4;
            if (left) z = upper ? SNAP_TL : lower ? SNAP_BL : SNAP_LEFT;
            else if (right) z = upper ? SNAP_TR : lower ? SNAP_BR : SNAP_RIGHT;
            else if (top) z = SNAP_MAX;
            if (z != snap_zone) {
                damage_snap(snap_zone);
                snap_zone = z;
                damage_snap(snap_zone);
            }
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
            } else if (snap_zone) {
                int z = snap_zone;
                damage_snap(z);
                snap_zone = 0;
                // Its size before the drag is what it gets back later
                w->sx = w->x;
                w->sy = w->y;
                w->sw = w->w;
                w->sh = w->h;
                w->tiled = 0;
                tile(drag_win, z);
            } else {
                vlog("window \"%s\" moved to %d,%d\n", w->title, w->x, w->y);
            }
            drag = DRAG_NONE;
        }
        return;
    }

    // A drag between windows: the label follows the pointer; on release
    // the window under it gets the drop
    if (dnd_active && moved) {
        int nx = cx, ny = cy;
        cx = ox; cy = oy;
        damage_dnd();
        cx = nx; cy = ny;
        damage_dnd();
    }
    if (dnd_active && !(buttons & 1)) {
        damage_dnd();
        dnd_active = 0;
        if (grab_win >= 0 && wins[grab_win].used) pointer_to(grab_win, RX_WINDOW_PTR_BUTTON, 0);
        grab_win = -1;
        int t, e2 = 0;
        if (hit(cx, cy, &t, &e2) == HIT_CONTENT && t >= 0 && !wins[t].panel) {
            static char out[sizeof(rx_window_event_t) + RX_WINDOW_MAX_CLIPBOARD];
            rx_window_event_t ev;
            memset(&ev, 0, sizeof ev);
            ev.op = RX_WINDOW_EV_DROP;
            ev.x = cx - wins[t].x;
            ev.y = cy - wins[t].y;
            ev.flags = t == dnd_src ? 1u : 0u;
            ev.n = (uint32_t)dnd_len;
            memcpy(out, &ev, sizeof ev);
            memcpy(out + sizeof ev, dnd_text, dnd_len);
            rx_channel_write(wins[t].ch, 0, out, (uint32_t)(sizeof ev + dnd_len), 0, 0);
            vlog("drop on \"%s\": %s\n", wins[t].title, dnd_label);
            if (t != dnd_src && !wins[t].desktop) {
                raise(t);
                set_focus(t);
            }
        } else {
            vlog("drop on nothing\n");
        }
        dnd_src = -1;
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

    // The desktop's menu: it takes the pointer while it is open
    if (menu_open) {
        int i = menu_item_at(cx, cy);
        if (i != menu_hover) {
            menu_hover = i;
            damage_menu();
        }
        if (pressed) {
            damage_menu();
            menu_open = 0;
            if (i >= 0) menu_choose(i);
        }
        return;
    }
    int k, edges = 0, where = hit(cx, cy, &k, &edges);
    if ((pressed & 2) && where == HIT_NONE && session_user[0]) {
        menu_open = 1;
        menu_hover = -1;
        menu_x = cx + MENU_W <= (int)PW ? cx : (int)PW - MENU_W;
        menu_y = cy + MENU_N * MENU_ROW + 8 <= (int)PH ? cy : cy - (MENU_N * MENU_ROW + 8);
        if (menu_y < 0) menu_y = 0;
        damage_menu();
        vlog("desktop menu at %d,%d\n", menu_x, menu_y);
        return;
    }
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
    // (and the panel, which is never focused); the wheel scrolls the window
    // under the pointer, focused or not
    if (where == HIT_CONTENT && (k == focus || wins[k].panel || (e->wheel && !pressed && !released))) {
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
        if (order[i] != focus && !wins[order[i]].panel && !wins[order[i]].desktop) {
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

// The volume keys (HID 0x7F Mute, 0x80 Volume Up, 0x81 Volume Down; PS/2
// keyboards' E0 20, E0 30, E0 2E): soundd's volume, and a notification
// saying it (the last one replaced)
static void volume_key(uint32_t code) {
    char out[512];
    const char *req = code == 0x7F ? "toggle" : code == 0x80 ? "volume +5" : "volume -5";
    if (rx_sound_control(req, out, sizeof out) < 0) return;
    int v = 0, m = 0;
    char *p = strstr(out, "volume ");
    if (p) v = atoi(p + 7);
    m = strstr(out, "muted yes") != 0;
    char t[64];
    int n = m ? snprintf(t, sizeof t, "Volume: muted") : snprintf(t, sizeof t, "Volume %d%%", v);
    // One volume notification at a time
    for (int i = 0; i < ntoasts; i++)
        if (!strncmp(toasts[i].text, "Volume", 6)) {
            memmove(&toasts[i], &toasts[i + 1], sizeof toasts[0] * (size_t)(ntoasts - i - 1));
            ntoasts--;
            i--;
        }
    notify(t, (uint32_t)n);
    add_damage((int)PW - TOAST_W - 16, 16, TOAST_W, MAX_TOASTS * (TOAST_H + 8));
}

// The Super key pressed and released alone: the panel opens or closes
// the launcher (RX_WINDOW_EV_DESKTOP, RX_WINDOW_DESKTOP_LAUNCHER)
static int super_alone;

static void key_event(const rx_input_event_t *e) {
    int pressed = e->flags & RX_INPUT_PRESSED;
    if (locked) {
        super_alone = 0;
        lock_key(e);
        return;
    }
    if (e->code == 0xE3 || e->code == 0xE7) {
        if (pressed && !(e->flags & RX_INPUT_REPEAT)) {
            super_alone = 1;
        } else if (!pressed && super_alone) {
            super_alone = 0;
            vlog("super: launcher\n");
            for (int k = 0; k < MAX_WINDOWS; k++)
                if (wins[k].used && wins[k].panel) send_simple(&wins[k], RX_WINDOW_EV_DESKTOP, RX_WINDOW_DESKTOP_LAUNCHER);
        }
    } else if (pressed) {
        super_alone = 0; // Super+L and the like
    }
    if (pressed && (e->code == 0x7F || e->code == 0x80 || e->code == 0x81)) {
        if (e->code != 0x7F || !(e->flags & RX_INPUT_REPEAT)) volume_key(e->code);
        return;
    }
    // Super+L (or Ctrl+Alt+L): lock
    if (pressed && e->code == 0x0F && ((e->mods & (RX_INPUT_MOD_LGUI | RX_INPUT_MOD_RGUI)) ||
                                       ((e->mods & (RX_INPUT_MOD_LCTRL | RX_INPUT_MOD_RCTRL)) && (e->mods & (RX_INPUT_MOD_LALT | RX_INPUT_MOD_RALT))))) {
        lock_screen();
        return;
    }
    // Super+arrows: tile left or right, maximise, restore (step F13)
    if (pressed && (e->mods & (RX_INPUT_MOD_LGUI | RX_INPUT_MOD_RGUI)) && focus >= 0 && !wins[focus].panel && !wins[focus].desktop) {
        window_t *f = &wins[focus];
        int z = -1;
        if (e->code == 0x50) z = f->tiled == SNAP_RIGHT ? SNAP_NONE : SNAP_LEFT;
        else if (e->code == 0x4F) z = f->tiled == SNAP_LEFT ? SNAP_NONE : SNAP_RIGHT;
        else if (e->code == 0x52) z = SNAP_MAX;
        else if (e->code == 0x51) z = (f->maximized || f->tiled) ? SNAP_NONE : -2;
        if (z == -2) {
            minimize(focus);
            return;
        }
        if (z >= 0) {
            tile(focus, z);
            return;
        }
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
        last_input = rx_clock_get_monotonic();
        if (saver_on) {
            // Any input ends the screensaver (and does nothing else)
            saver_on = 0;
            vlog("screensaver off\n");
            add_damage(0, 0, (int)W, (int)H);
            continue;
        }
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
    use_theme(&theme_builtin);
    last_input = rx_clock_get_monotonic();
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
    rx_display_primary(&d, &PW, &PH);
    fbp = d.pixels;
    cx = (int)PW / 2;
    cy = (int)PH / 2;
    load_appearance(1);
    setup_hwcursor();
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
        // The screensaver's time
        uint64_t idle_ns = (uint64_t)idle_minutes * 60000000000ull;
        if (idle_minutes && !saver_on && visible && last_input + idle_ns < deadline) deadline = last_input + idle_ns;
        // At least once a second: the appearance may have changed
        if (deadline > rx_clock_get_monotonic() + 1000000000ull) deadline = rx_clock_get_monotonic() + 1000000000ull;
        rx_object_wait_many(items, (uint64_t)n, deadline);
        load_appearance(0);

        if (i_ctl >= 0 && items[i_ctl].pending) {
            // The desktop: "user NAME" or "user"; when it ends, so do we
            char msg[80];
            uint32_t nb = 0;
            rx_status_t cs;
            while ((cs = rx_channel_read(control, 0, msg, 0, sizeof msg - 1, 0, &nb, 0)) == RX_OK) {
                msg[nb] = 0;
                if (!strncmp(msg, "user", 4)) {
                    set_session_user(msg[4] == ' ' ? msg + 5 : "");
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
            while ((pr = rx_display_poll(&d, &ev)) > 0) {
                if (ev.op == RX_DISPLAY_EV_VISIBLE) {
                    visible = (int)ev.a;
                    last_input = rx_clock_get_monotonic();
                    if (visible) {
                        add_damage(0, 0, (int)W, (int)H);
                        if (hwcursor) rx_display_cursor_move(&d, cx, cy, 1);
                    }
                }
                if (ev.op == RX_DISPLAY_EV_RESIZE) screen_resized();
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
        if (idle_minutes && !saver_on && visible && session_user[0] && now >= last_input + idle_ns) {
            saver_on = 1;
            vlog("screensaver on (%s, after %d minutes)\n", saver_kind, idle_minutes);
            if (idle_lock) lock_screen();
            add_damage(0, 0, (int)W, (int)H);
        }
        expire_toasts(now);
        for (int k = 0; k < MAX_WINDOWS; k++)
            if (wins[k].used && wins[k].close_deadline && now >= wins[k].close_deadline)
                remove_window(k, "it did not close when asked");
        update_cursor_shape();
        compose();
    }
}
