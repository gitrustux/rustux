/*
 * libnsfb's surface for Rustica: a window of the Rustux compositor
 * (<rustux/window.h>). Written for Rustica (2026-11-26, phase W); MIT, as
 * libnsfb.
 *
 * The window's buffer is the framebuffer (32 bits a pixel, 0x00RRGGBB:
 * libnsfb's XRGB8888); update commits the changed box; the compositor
 * draws the pointer, so the cursor routine does nothing. Keys come as
 * HID usages and are mapped to libnsfb's codes; a press that makes text
 * in the user's keyboard layout comes as RUSTICA_TEXT_KEY + the character
 * (the framebuffer frontend's fbtk_keycode_to_ucs4 takes it as it is: a
 * Rustica change there), so every layout types what it should.
 */
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <rustux/input.h>
#include <rustux/syscalls.h>
#include <rustux/window.h>

#include "libnsfb.h"
#include "libnsfb_event.h"
#include "libnsfb_plot.h"

#include "nsfb.h"
#include "plot.h"
#include "surface.h"

#define RUSTICA_TEXT_KEY 0x10000

struct rustica {
    rx_window_t win;
    int open;
    uint32_t buttons;
    nsfb_event_t queue[16];
    int qhead, qlen;
    unsigned shape;     /* the pointer's shape last asked for */
};

static void push(struct rustica *r, enum nsfb_event_type_e type, int code) {
    if (r->qlen == 16) return;
    nsfb_event_t *e = &r->queue[(r->qhead + r->qlen++) % 16];
    memset(e, 0, sizeof *e);
    e->type = type;
    e->value.keycode = code;
}

static nsfb_event_t *push_ev(struct rustica *r) {
    if (r->qlen == 16) return NULL;
    nsfb_event_t *e = &r->queue[(r->qhead + r->qlen++) % 16];
    memset(e, 0, sizeof *e);
    return e;
}

/* HID usage (keyboard page) to libnsfb's key code */
static int hid_to_nsfb(uint32_t u) {
    if (u >= 0x04 && u <= 0x1D) return NSFB_KEY_a + (int)(u - 0x04);
    if (u >= 0x1E && u <= 0x26) return NSFB_KEY_1 + (int)(u - 0x1E);
    if (u >= 0x3A && u <= 0x45) return NSFB_KEY_F1 + (int)(u - 0x3A);
    if (u >= 0x59 && u <= 0x61) return NSFB_KEY_KP1 + (int)(u - 0x59);
    switch (u) {
    case 0x27: return NSFB_KEY_0;
    case 0x28: return NSFB_KEY_RETURN;
    case 0x29: return NSFB_KEY_ESCAPE;
    case 0x2A: return NSFB_KEY_BACKSPACE;
    case 0x2B: return NSFB_KEY_TAB;
    case 0x2C: return NSFB_KEY_SPACE;
    case 0x2D: return NSFB_KEY_MINUS;
    case 0x2E: return NSFB_KEY_EQUALS;
    case 0x2F: return NSFB_KEY_LEFTBRACKET;
    case 0x30: return NSFB_KEY_RIGHTBRACKET;
    case 0x31: return NSFB_KEY_BACKSLASH;
    case 0x33: return NSFB_KEY_SEMICOLON;
    case 0x34: return NSFB_KEY_QUOTE;
    case 0x35: return NSFB_KEY_BACKQUOTE;
    case 0x36: return NSFB_KEY_COMMA;
    case 0x37: return NSFB_KEY_PERIOD;
    case 0x38: return NSFB_KEY_SLASH;
    case 0x39: return NSFB_KEY_CAPSLOCK;
    case 0x47: return NSFB_KEY_SCROLLOCK;
    case 0x48: return NSFB_KEY_PAUSE;
    case 0x49: return NSFB_KEY_INSERT;
    case 0x4A: return NSFB_KEY_HOME;
    case 0x4B: return NSFB_KEY_PAGEUP;
    case 0x4C: return NSFB_KEY_DELETE;
    case 0x4D: return NSFB_KEY_END;
    case 0x4E: return NSFB_KEY_PAGEDOWN;
    case 0x4F: return NSFB_KEY_RIGHT;
    case 0x50: return NSFB_KEY_LEFT;
    case 0x51: return NSFB_KEY_DOWN;
    case 0x52: return NSFB_KEY_UP;
    case 0x53: return NSFB_KEY_NUMLOCK;
    case 0x54: return NSFB_KEY_KP_DIVIDE;
    case 0x55: return NSFB_KEY_KP_MULTIPLY;
    case 0x56: return NSFB_KEY_KP_MINUS;
    case 0x57: return NSFB_KEY_KP_PLUS;
    case 0x58: return NSFB_KEY_KP_ENTER;
    case 0x62: return NSFB_KEY_KP0;
    case 0x63: return NSFB_KEY_KP_PERIOD;
    case 0x65: return NSFB_KEY_MENU;
    case 0xE0: return NSFB_KEY_LCTRL;
    case 0xE1: return NSFB_KEY_LSHIFT;
    case 0xE2: return NSFB_KEY_LALT;
    case 0xE3: return NSFB_KEY_LSUPER;
    case 0xE4: return NSFB_KEY_RCTRL;
    case 0xE5: return NSFB_KEY_RSHIFT;
    case 0xE6: return NSFB_KEY_RALT;
    case 0xE7: return NSFB_KEY_RSUPER;
    }
    return NSFB_KEY_UNKNOWN;
}

/* The first character of UTF-8 text, or -1 */
static int first_char(const char *s) {
    const unsigned char *p = (const unsigned char *)s;
    if (p[0] < 0x80) return p[0] ? p[0] : -1;
    if ((p[0] & 0xE0) == 0xC0 && p[1]) return ((p[0] & 0x1F) << 6) | (p[1] & 0x3F);
    if ((p[0] & 0xF0) == 0xE0 && p[1] && p[2]) return ((p[0] & 0x0F) << 12) | ((p[1] & 0x3F) << 6) | (p[2] & 0x3F);
    if ((p[0] & 0xF8) == 0xF0 && p[1] && p[2] && p[3])
        return ((p[0] & 0x07) << 18) | ((p[1] & 0x3F) << 12) | ((p[2] & 0x3F) << 6) | (p[3] & 0x3F);
    return -1;
}

static int rustica_defaults(nsfb_t *nsfb) {
    nsfb->width = 1100;
    nsfb->height = 680;
    nsfb->format = NSFB_FMT_XRGB8888;
    nsfb->bpp = 32;
    select_plotters(nsfb);
    return 0;
}

static int rustica_initialise(nsfb_t *nsfb) {
    struct rustica *r = calloc(1, sizeof *r);
    if (!r) return -1;
    if (rx_window_create(&r->win, -1, -1, (uint32_t)nsfb->width, (uint32_t)nsfb->height, "NetSurf") < 0) {
        free(r);
        return -1;
    }
    r->open = 1;
    nsfb->surface_priv = r;
    nsfb->format = NSFB_FMT_XRGB8888;
    nsfb->bpp = 32;
    nsfb->ptr = (uint8_t *)r->win.pixels;
    nsfb->linelen = (int)r->win.width * 4;
    nsfb->width = (int)r->win.width;
    nsfb->height = (int)r->win.height;
    select_plotters(nsfb);
    return 0;
}

static int rustica_geometry(nsfb_t *nsfb, int width, int height, enum nsfb_format_e format) {
    struct rustica *r = nsfb->surface_priv;
    (void)format;
    if (width <= 0) width = nsfb->width;
    if (height <= 0) height = nsfb->height;
    if (r && r->open && ((uint32_t)width != r->win.width || (uint32_t)height != r->win.height)) {
        if (rx_window_resize(&r->win, (uint32_t)width, (uint32_t)height) < 0) return -1;
        nsfb->ptr = (uint8_t *)r->win.pixels;
        nsfb->linelen = (int)r->win.width * 4;
    }
    nsfb->width = width;
    nsfb->height = height;
    nsfb->format = NSFB_FMT_XRGB8888;
    nsfb->bpp = 32;
    select_plotters(nsfb);
    return 0;
}

static int rustica_finalise(nsfb_t *nsfb) {
    struct rustica *r = nsfb->surface_priv;
    if (r) {
        if (r->open) rx_window_destroy(&r->win);
        free(r);
    }
    nsfb->surface_priv = NULL;
    nsfb->ptr = NULL;
    return 0;
}

/* One compositor event as libnsfb events (queued) */
static void translate(struct rustica *r, const rx_window_event_t *ev) {
    nsfb_event_t *e;
    switch (ev->op) {
    case RX_WINDOW_EV_CONFIGURE:
        if ((e = push_ev(r))) {
            e->type = NSFB_EVENT_RESIZE;
            e->value.resize.w = (int)ev->w;
            e->value.resize.h = (int)ev->h;
        }
        break;
    case RX_WINDOW_EV_KEY: {
        int code = hid_to_nsfb(ev->code);
        int down = (ev->flags & RX_INPUT_PRESSED) != 0;
        int ch = first_char(ev->text);
        int ctrl_alt = (ev->mods & (RX_INPUT_MOD_LCTRL | RX_INPUT_MOD_RCTRL | RX_INPUT_MOD_LALT)) != 0;
        if (down && !ctrl_alt && ch >= 0x20 && ch != 0x7F) {
            /* text in the user's layout */
            push(r, NSFB_EVENT_KEY_DOWN, RUSTICA_TEXT_KEY + ch);
            if (code != NSFB_KEY_UNKNOWN && (ev->flags & RX_INPUT_REPEAT) == 0) {
                /* nothing else: the release below says it went up */
            }
        } else if (code != NSFB_KEY_UNKNOWN) {
            push(r, down ? NSFB_EVENT_KEY_DOWN : NSFB_EVENT_KEY_UP, code);
        }
        break;
    }
    case RX_WINDOW_EV_POINTER:
        if (ev->flags & RX_WINDOW_PTR_MOTION) {
            if ((e = push_ev(r))) {
                e->type = NSFB_EVENT_MOVE_ABSOLUTE;
                e->value.vector.x = ev->x;
                e->value.vector.y = ev->y;
            }
        }
        if (ev->flags & RX_WINDOW_PTR_BUTTON) {
            static const int codes[3] = { NSFB_KEY_MOUSE_1, NSFB_KEY_MOUSE_3, NSFB_KEY_MOUSE_2 };
            /* buttons: bit 0 left, 1 right, 2 middle (as <rustux/input.h>) */
            for (int b = 0; b < 3; b++) {
                uint32_t m = 1u << b;
                if ((ev->buttons & m) != (r->buttons & m)) {
                    if ((e = push_ev(r))) {
                        e->type = (ev->buttons & m) ? NSFB_EVENT_KEY_DOWN : NSFB_EVENT_KEY_UP;
                        e->value.keycode = codes[b];
                    }
                }
            }
            r->buttons = ev->buttons;
        }
        if ((ev->flags & RX_WINDOW_PTR_WHEEL) && ev->wheel) {
            int code = ev->wheel > 0 ? NSFB_KEY_MOUSE_4 : NSFB_KEY_MOUSE_5;
            int n = ev->wheel > 0 ? ev->wheel : -ev->wheel;
            for (int i = 0; i < n && i < 4; i++) {
                push(r, NSFB_EVENT_KEY_DOWN, code);
                push(r, NSFB_EVENT_KEY_UP, code);
            }
        }
        break;
    case RX_WINDOW_EV_CLOSE_REQUEST:
        if ((e = push_ev(r))) {
            e->type = NSFB_EVENT_CONTROL;
            e->value.controlcode = NSFB_CONTROL_QUIT;
        }
        break;
    case RX_WINDOW_EV_WAKE:
        if ((e = push_ev(r))) {
            e->type = NSFB_EVENT_CONTROL;
            e->value.controlcode = NSFB_CONTROL_TIMEOUT;
        }
        break;
    default:
        break;
    }
}

static bool rustica_input(nsfb_t *nsfb, nsfb_event_t *event, int timeout) {
    struct rustica *r = nsfb->surface_priv;
    if (!r) return false;
    uint64_t deadline = timeout < 0 ? RX_TIME_INFINITE
                        : timeout == 0 ? 0
                                       : rx_clock_get_monotonic() + (uint64_t)timeout * 1000000ull;
    while (r->qlen == 0) {
        rx_window_event_t ev;
        int got = rx_window_next_event(&r->win, &ev, deadline);
        if (got < 0) {
            /* the compositor is gone */
            event->type = NSFB_EVENT_CONTROL;
            event->value.controlcode = NSFB_CONTROL_QUIT;
            return true;
        }
        if (got == 0) {
            if (timeout == 0) return false;
            event->type = NSFB_EVENT_CONTROL;
            event->value.controlcode = NSFB_CONTROL_TIMEOUT;
            return true;
        }
        translate(r, &ev);
    }
    *event = r->queue[r->qhead];
    r->qhead = (r->qhead + 1) % 16;
    r->qlen--;
    return true;
}

static int rustica_claim(nsfb_t *nsfb, nsfb_bbox_t *box) {
    (void)nsfb;
    (void)box;
    return 0;
}

static int rustica_update(nsfb_t *nsfb, nsfb_bbox_t *box) {
    struct rustica *r = nsfb->surface_priv;
    if (!r || !r->open) return 0;
    rx_window_rect_t rc;
    int x0 = box->x0 < 0 ? 0 : box->x0, y0 = box->y0 < 0 ? 0 : box->y0;
    int x1 = box->x1 > nsfb->width ? nsfb->width : box->x1, y1 = box->y1 > nsfb->height ? nsfb->height : box->y1;
    if (x1 <= x0 || y1 <= y0) return 0;
    rc.x = x0;
    rc.y = y0;
    rc.w = (uint32_t)(x1 - x0);
    rc.h = (uint32_t)(y1 - y0);
    rx_window_commit(&r->win, &rc, 1);
    return 0;
}

static int rustica_cursor(nsfb_t *nsfb, struct nsfb_cursor_s *cursor) {
    /* the compositor draws the pointer */
    (void)nsfb;
    (void)cursor;
    return true;
}

/* The compositor's pointer shape over the window (RX_WINDOW_CURSOR_*),
 * for programs that know they run on Rustica (NetSurf: the hand over
 * links). False if this is not a Rustica surface. */
bool nsfb_rustica_cursor_shape(nsfb_t *nsfb, unsigned shape);
bool nsfb_rustica_cursor_shape(nsfb_t *nsfb, unsigned shape) {
    /* libnsfb keeps a copy of the routines: the surface is ours if its
     * input routine is */
    if (nsfb == NULL || nsfb->surface_rtns == NULL || nsfb->surface_rtns->input != rustica_input || nsfb->surface_priv == NULL)
        return false;
    struct rustica *r = nsfb->surface_priv;
    if (r->shape != shape) {
        r->shape = shape;
        rx_window_set_cursor(&r->win, shape);
    }
    return true;
}

const nsfb_surface_rtns_t rustica_rtns = {
    .defaults = rustica_defaults,
    .initialise = rustica_initialise,
    .finalise = rustica_finalise,
    .input = rustica_input,
    .claim = rustica_claim,
    .update = rustica_update,
    .cursor = rustica_cursor,
    .geometry = rustica_geometry,
};

NSFB_SURFACE_DEF(rustica, NSFB_SURFACE_RUSTICA, &rustica_rtns)
