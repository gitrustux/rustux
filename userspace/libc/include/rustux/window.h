// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The window protocol, /svc/window (DESKTOP.md step F4, docs/WINDOWS.md).
//
// The compositor owns the screen (through the display service) and the
// input while graphics are shown (from the input service). A client opens
// one channel per window and sends CREATE with a buffer: a VMO of w x h
// pixels, 0x00RRGGBB, w * 4 bytes a row. It draws into the buffer and
// sends COMMIT with the damaged rectangles; the compositor copies them
// (from then on the client may draw again) and sends FRAME once they are
// on the screen, which paces animation: draw, commit, wait for FRAME.
//
// The compositor draws the title bar and border, moves, resizes,
// minimises, maximises, raises and focuses windows. It asks the client to
// change size with CONFIGURE; the client answers with BUFFER (a VMO of the
// new size) and a COMMIT. Input goes only to the focused window, with
// pointer coordinates relative to its content. CLOSE_REQUEST asks the
// client to close (the close button, Alt+F4); a client that does not
// within 5 seconds loses its window. Closing the channel removes the
// window.
//
// Client to compositor: rx_window_req_t, then the payload (CREATE and
// SET_TITLE: the title; COMMIT: rectangles; CLIPBOARD_SET: text). Only
// CREATE and CLIPBOARD_GET are answered (op | RX_WINDOW_REPLY).
// Compositor to client: rx_window_event_t, a CLIPBOARD_GET reply followed
// by the text.

#ifndef RUSTUX_WINDOW_H
#define RUSTUX_WINDOW_H

#include <stddef.h>
#include <rustux/syscalls.h>

typedef struct {
    uint32_t op;
    uint32_t n;         // COMMIT: rectangles; payload bytes otherwise
    int32_t x, y;       // CREATE: where the content goes (-1, -1: the compositor chooses)
    uint32_t w, h;      // CREATE, BUFFER: the buffer's size
    uint32_t flags;     // SET_FULLSCREEN: 1 on, 0 off
} rx_window_req_t;

typedef struct {
    uint32_t x, y, w, h;
} rx_window_rect_t;

#define RX_WINDOW_REPLY          0x80000000u

// Requests
#define RX_WINDOW_CREATE         1u  // + title; the buffer VMO attached; reply: status, w, h
#define RX_WINDOW_COMMIT         2u  // + n rectangles (0: everything)
#define RX_WINDOW_BUFFER         3u  // a new buffer (after CONFIGURE), VMO attached
#define RX_WINDOW_SET_TITLE      4u  // + title
#define RX_WINDOW_CLIPBOARD_SET  5u  // + text (UTF-8)
#define RX_WINDOW_CLIPBOARD_GET  6u  // reply: event with n = bytes, then the text
#define RX_WINDOW_SET_FULLSCREEN 7u  // flags 1 or 0
// The desktop session (step F6)
#define RX_WINDOW_SET_ROLE       8u  // flags RX_WINDOW_ROLE_PANEL: docked at the bottom, on top, never focused
#define RX_WINDOW_ACTIVATE       9u  // (the panel) x = a window's id: focus and raise it, or minimise it if focused
#define RX_WINDOW_NOTIFY         10u // + text: a notification (also as a connection's first message, without a window)
#define RX_WINDOW_LOCK           11u // lock the screen (the session's user's password unlocks it)
#define RX_WINDOW_ROLE_PANEL     1u

#define RX_WINDOW_MAX_TITLE      64
#define RX_WINDOW_MAX_RECTS      64
#define RX_WINDOW_MAX_CLIPBOARD  4000

// Events
#define RX_WINDOW_EV_CONFIGURE     101u  // w, h: the size the compositor wants
#define RX_WINDOW_EV_KEY           102u  // code (HID usage), mods, flags (input.h), text
#define RX_WINDOW_EV_POINTER       103u  // x, y in the content, buttons, wheel, flags below
#define RX_WINDOW_EV_FOCUS         104u  // flags 1 focused, 0 not
#define RX_WINDOW_EV_CLOSE_REQUEST 105u
#define RX_WINDOW_EV_FRAME         106u  // the last commit is on the screen
#define RX_WINDOW_EV_STATE         107u  // flags: RX_WINDOW_STATE_*
// To the panel: x -1 starts a new list, then one per window: x its id,
// flags 1 focused, 2 minimised, text its title (cut to 15 characters)
#define RX_WINDOW_EV_WINDOW_INFO   108u
// Not from the compositor: rx_window_wake was called (from another thread)
#define RX_WINDOW_EV_WAKE          109u
// The theme or the wallpaper changed: text = the theme's name ("rustica",
// "dracula"); clients redraw in it
#define RX_WINDOW_EV_APPEARANCE    110u
// To the panel: the desktop's menu was used (flags 1 change the wallpaper,
// 2 the appearance settings)
#define RX_WINDOW_EV_DESKTOP       111u

// Pointer event flags
#define RX_WINDOW_PTR_MOTION  0x01u
#define RX_WINDOW_PTR_BUTTON  0x02u  // a button went down or up (see buttons)
#define RX_WINDOW_PTR_WHEEL   0x04u
#define RX_WINDOW_PTR_LEAVE   0x08u  // the pointer left the content

// Window states
#define RX_WINDOW_STATE_MAXIMIZED  0x01u
#define RX_WINDOW_STATE_FULLSCREEN 0x02u
#define RX_WINDOW_STATE_MINIMIZED  0x04u

typedef struct {
    uint32_t op;
    int32_t status;     // replies
    int32_t x, y;
    uint32_t w, h;
    uint32_t code, mods, flags, buttons;
    int32_t wheel;
    uint32_t n;         // CLIPBOARD_GET reply: bytes of text that follow
    char text[16];      // EV_KEY: what the layout makes of a press
    uint64_t time;
} rx_window_event_t;

// ---- the client library (libc window.c) ----

typedef struct {
    rx_handle_t ch, vmo;
    uint32_t *pixels;       // the buffer, mapped: width x height, a row is `width` pixels
    uint32_t width, height;
    int focused, frame_pending;
    rx_window_event_t queue[64];
    int qhead, qlen;
} rx_window_t;

// A window of w x h pixels (at x, y on the screen, or -1, -1): 0 or a
// negative errno (ENOENT: no compositor)
int rx_window_create(rx_window_t *win, int x, int y, uint32_t w, uint32_t h, const char *title);
// Show these rectangles of the buffer (n 0: all of it)
int rx_window_commit(rx_window_t *win, const rx_window_rect_t *rects, int n);
// Wait for the FRAME of the last commit (other events are queued)
int rx_window_wait_frame(rx_window_t *win, uint64_t deadline);
// The next event, waiting until deadline (RX_TIME_INFINITE, or 0: do not
// wait): 1, 0 (none) or a negative errno (the compositor is gone)
int rx_window_next_event(rx_window_t *win, rx_window_event_t *ev, uint64_t deadline);
// After EV_CONFIGURE: a new buffer of w x h (cleared to black); draw and commit
int rx_window_resize(rx_window_t *win, uint32_t w, uint32_t h);
int rx_window_set_title(rx_window_t *win, const char *title);
int rx_window_set_fullscreen(rx_window_t *win, int on);
int rx_window_clipboard_set(rx_window_t *win, const char *text);
// The clipboard's text into buf (0-terminated): its length or a negative errno
int rx_window_clipboard_get(rx_window_t *win, char *buf, size_t size);
void rx_window_destroy(rx_window_t *win);
// The desktop session (step F6)
int rx_window_set_role(rx_window_t *win, uint32_t role);
int rx_window_activate(rx_window_t *win, int id);
int rx_window_lock(rx_window_t *win);
// A notification, with or without a window (win 0: a connection of its own)
int rx_window_notify(rx_window_t *win, const char *text);
// From any thread: make the window's rx_window_next_event return
// RX_WINDOW_EV_WAKE (ch is the window's ch; step F7)
int rx_window_wake(rx_handle_t ch);

#endif
