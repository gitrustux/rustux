// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The input service's protocol, /svc/input (DESKTOP.md step F3,
// docs/INPUT.md).
//
// inputd reads every keyboard and pointing device (the kernel's USB
// keyboards, mice and tablets; /bin/ps2kbd's PS/2 keyboard and mouse),
// applies the keyboard layout (/etc/keymap), repeats held keys, and gives
// the events to one consumer at a time, the one with the focus:
//
//   - a program that grabbed the input (GRAB: root; evtest), until it
//     lets go or ends;
//   - otherwise, while graphics are on the screen, the program that
//     registered for them (GRAPHICS: the compositor), or else the display
//     service's owner (through the display service);
//   - otherwise the text console (init).
//
// Ctrl+Alt+F1 and Ctrl+Alt+F7 are inputd's own: they switch between the
// text console and graphics. No program reads keys it does not have the
// focus for.
//
// Requests are an rx_input_req_t; every one gets an int32_t reply (0 or a
// negative errno). Events to a grabbing client are rx_input_event_t.

#ifndef RUSTUX_INPUT_H
#define RUSTUX_INPUT_H

#include <stdint.h>

typedef struct {
    uint32_t op;
    uint32_t arg;
} rx_input_req_t;

// Take the focus until the channel closes or UNGRAB (root; EBUSY if
// another program has it)
#define RX_INPUT_OP_GRAB          1u
#define RX_INPUT_OP_UNGRAB        2u
// Read /etc/keymap again (anyone: loadkeys)
#define RX_INPUT_OP_RELOAD_KEYMAP 3u
// Restart the PS/2 driver (root)
#define RX_INPUT_OP_RESTART_KBD   4u
// Receive the events while graphics are on the screen (root: the
// compositor, step F4), until the channel closes; instead of the display
// service's owner
#define RX_INPUT_OP_GRAPHICS      5u

#define RX_INPUT_KEY     1u
#define RX_INPUT_POINTER 2u

// Modifier bits (HID): left Ctrl, Shift, Alt, GUI, then the right ones
#define RX_INPUT_MOD_LCTRL  0x01u
#define RX_INPUT_MOD_LSHIFT 0x02u
#define RX_INPUT_MOD_LALT   0x04u
#define RX_INPUT_MOD_LGUI   0x08u
#define RX_INPUT_MOD_RCTRL  0x10u
#define RX_INPUT_MOD_RSHIFT 0x20u
#define RX_INPUT_MOD_RALT   0x40u
#define RX_INPUT_MOD_RGUI   0x80u

// Key flags
#define RX_INPUT_PRESSED 0x01u  // pressed (else released)
#define RX_INPUT_REPEAT  0x04u  // a repeat of a held key (made by inputd)

// Pointer flags
#define RX_INPUT_ABSOLUTE 0x01u // x, y are 0..32767 across the screen

typedef struct {
    uint32_t type;      // RX_INPUT_KEY or RX_INPUT_POINTER
    uint32_t code;      // KEY: HID usage (page 7); POINTER: buttons (bit 0 left, 1 right, 2 middle)
    uint32_t mods;      // KEY: modifier bits
    uint32_t flags;     // KEY: RX_INPUT_PRESSED, RX_INPUT_REPEAT; POINTER: RX_INPUT_ABSOLUTE
    int32_t x, y;       // POINTER: absolute, or relative motion (y down)
    int32_t wheel;      // POINTER: + away from the user
    char text[12];      // KEY, when pressed: what the layout makes of it (UTF-8 or an escape sequence), 0-terminated
    uint64_t time;      // monotonic ns
} rx_input_event_t;

// inputd to the console (init): one message per key press or repeat
typedef struct {
    uint8_t usage, mods, flags, len;
    char text[12];      // the bytes for the terminal; Enter is "\r"
} rx_input_console_t;

#endif
