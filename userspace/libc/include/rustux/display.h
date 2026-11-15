// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The display service's protocol, /svc/display (DESKTOP.md step F2,
// docs/DISPLAY.md).
//
// The service owns the framebuffer. One client at a time, the owner,
// draws: ACQUIRE gives it a back buffer in RAM (a VMO of width x height
// pixels, 0x00RRGGBB, `width * 4` bytes a row) and puts the graphics on
// the screen; PRESENT copies the damaged rectangles of the back buffer to
// the screen and replies once they are there, so an owner that waits for
// the reply before drawing again never shows a half-drawn frame. RELEASE,
// or closing the channel, gives the screen back to the text console.
//
// Ctrl+Alt+F1 shows the text console, Ctrl+Alt+F7 the owner's graphics
// again (init asks the service with SWITCH); the owner hears of it as an
// RX_DISPLAY_EV_VISIBLE event, and while its graphics are not shown its
// PRESENTs only update the back buffer. While they are shown, init sends
// the keys to the service, which passes them to the owner as
// RX_DISPLAY_EV_KEY events (until the input service, step F3).
//
// Messages are an rx_display_msg_t, PRESENT's followed by its rectangles.
// Every request but SWITCH and KEY gets a reply: the same op with
// RX_DISPLAY_REPLY set, `status` 0 or a negative errno. ACQUIRE and
// SNAPSHOT need root (EPERM), SWITCH and KEY too.

#ifndef RUSTUX_DISPLAY_H
#define RUSTUX_DISPLAY_H

#include <rustux/syscalls.h>

typedef struct {
    uint32_t op;
    int32_t status;     // replies
    uint32_t a, b;      // per op
} rx_display_msg_t;

typedef struct {
    uint32_t x, y, w, h;
} rx_display_rect_t;

#define RX_DISPLAY_REPLY        0x80000000u
#define RX_DISPLAY_EVENT        0x40000000u

// Reply: a = width, b = height; then a second word pair is not needed:
// the back buffer is always width * 4 bytes a row
#define RX_DISPLAY_OP_INFO      1u
// Reply carries the back buffer VMO (read, write, map); a = width, b = height
#define RX_DISPLAY_OP_ACQUIRE   2u
#define RX_DISPLAY_OP_RELEASE   3u
// a = the number of rectangles that follow (0: the whole screen, at most
// RX_DISPLAY_MAX_RECTS). Reply a = 1 if the graphics are on the screen
#define RX_DISPLAY_OP_PRESENT   4u
#define RX_DISPLAY_MAX_RECTS    64
// What is on the screen now (text console or graphics): reply carries a
// VMO of width x height 0x00RRGGBB pixels; a = width, b = height
#define RX_DISPLAY_OP_SNAPSHOT  5u
// a = RX_DISPLAY_TEXT or RX_DISPLAY_GRAPHICS (init, for Ctrl+Alt+F1/F7)
#define RX_DISPLAY_OP_SWITCH    6u
// a = usage | mods << 8 | flags << 16, as init's key events (init)
#define RX_DISPLAY_OP_KEY       7u

// A pointer event for the owner (init, step F3): a = buttons (bit 0 left,
// 1 right, 2 middle) | wheel (i8, + away from the user) << 8 | flags << 16
// (bit 0: absolute), b = x | y << 16: absolute 0..32767 across the screen,
// or relative (i16, y down)
#define RX_DISPLAY_OP_POINTER   8u

// Step F8 (virtio-gpu):
// Reply a = what the display can do: RX_DISPLAY_CAP_*
#define RX_DISPLAY_OP_CAPS        9u
#define RX_DISPLAY_CAP_CURSOR     1u  // a hardware cursor (CURSOR_SET, CURSOR_MOVE)
#define RX_DISPLAY_CAP_RESIZE     2u  // the size can change (EV_RESIZE)
// (the owner) the pointer's image: a VMO of 64 x 64 pixels 0xAARRGGBB
// attached; a = hot_x | hot_y << 16. ENOTSUP without a hardware cursor
#define RX_DISPLAY_OP_CURSOR_SET  10u
// (the owner) a = x | y << 16, b = 1 shown, 0 hidden; no reply
#define RX_DISPLAY_OP_CURSOR_MOVE 11u
// The first head's part of the screen, from the top left (several heads
// are one screen, side by side): reply a = width, b = height
#define RX_DISPLAY_OP_PRIMARY     12u

// Events to the owner (op), with no reply
#define RX_DISPLAY_EV_KEY       (RX_DISPLAY_EVENT | 1u)   // a as for OP_KEY
#define RX_DISPLAY_EV_VISIBLE   (RX_DISPLAY_EVENT | 2u)   // a = 1 shown, 0 not
#define RX_DISPLAY_EV_POINTER   (RX_DISPLAY_EVENT | 3u)   // a, b as for OP_POINTER
// The screen changed size (a host window was resized; step F8): a =
// width, b = height, a new back buffer VMO attached. The client library
// maps it in place of the old one before handing the event on; draw
// everything again and PRESENT it all
#define RX_DISPLAY_EV_RESIZE    (RX_DISPLAY_EVENT | 4u)

// Client side (libc display.c)
typedef struct {
    rx_handle_t ch;
    rx_handle_t vmo;
    uint32_t *pixels;       // the back buffer, mapped
    uint32_t width, height; // pixels; a row is `width` pixels
    int visible;
    uint32_t caps;          // RX_DISPLAY_CAP_* (step F8)
    // Events that arrived while waiting for a reply
    rx_display_msg_t queue[32];
    int qhead, qlen;
} rx_display_t;

// Connect, ACQUIRE and map the back buffer: 0 or a negative errno
// (ENOENT no display service, EBUSY another owner, EPERM not root)
int rx_display_open(rx_display_t *d);
// PRESENT these rectangles (n 0: everything) and wait until they are on
// the screen: 1 shown, 0 not shown (text console), or a negative errno
int rx_display_present(rx_display_t *d, const rx_display_rect_t *rects, int n);
// The next event: 1 (in *ev), 0 none, or a negative errno (the service ended)
int rx_display_poll(rx_display_t *d, rx_display_msg_t *ev);
void rx_display_close(rx_display_t *d);
// A hardware cursor (caps & RX_DISPLAY_CAP_CURSOR): its image (64 x 64
// 0xAARRGGBB) and hot spot, then its position. 0 or a negative errno
int rx_display_cursor_set(rx_display_t *d, const uint32_t *argb, uint32_t hot_x, uint32_t hot_y);
int rx_display_cursor_move(rx_display_t *d, int x, int y, int visible);
// The first head's width and height (the whole screen with one head)
int rx_display_primary(rx_display_t *d, uint32_t *w, uint32_t *h);
// A copy of the screen as it is (any client; root): 0 and *pixels
// (malloc'd, width x height 0x00RRGGBB), or a negative errno
int rx_display_snapshot(uint32_t **pixels, uint32_t *width, uint32_t *height);

#endif
