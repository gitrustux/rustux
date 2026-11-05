// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ps2kbd: the PS/2 keyboard driver, as an ordinary user program.
//
// init starts it with the root resource and a channel. It asks the kernel
// for the keyboard's two I/O ports (0x60 data, 0x64 status) and for IRQ 1,
// then loops: wait for the interrupt, read the scancodes, turn them into key
// events (HID usages; init applies the layout), send them over the
// channel. The kernel only set up the 8042
// controller at boot; from here on it never touches the keyboard.
//
// Exit codes: 0 channel closed, 2 no PS/2 controller (init does not
// restart it), 3 setup failed.

#include "rx.h"

#define DATA_PORT   0x60
#define STATUS_PORT 0x64
#define STATUS_OBF  0x01    // output buffer full: a byte is waiting
#define STATUS_AUX  0x20    // the byte is from the mouse
#define STATUS_ERR  0xC0    // parity / timeout error

// Scancode set 1 (what the 8042 delivers with translation on) to HID
// usages (page 0x07), the codes init's keyboard layout works with
static const uint8_t set1_to_hid[0x59] = {
    0x00, 0x29, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x2D, 0x2E, 0x2A, 0x2B,
    0x14, 0x1A, 0x08, 0x15, 0x17, 0x1C, 0x18, 0x0C, 0x12, 0x13, 0x2F, 0x30, 0x28, 0xE0, 0x04, 0x16,
    0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x33, 0x34, 0x35, 0xE1, 0x31, 0x1D, 0x1B, 0x06, 0x19,
    0x05, 0x11, 0x10, 0x36, 0x37, 0x38, 0xE5, 0x55, 0xE2, 0x2C, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E,
    0x3F, 0x40, 0x41, 0x42, 0x43, 0x53, 0x47, 0x5F, 0x60, 0x61, 0x56, 0x5C, 0x5D, 0x5E, 0x57, 0x59,
    0x5A, 0x5B, 0x62, 0x63, 0x00, 0x00, 0x64, 0x44, 0x45,
};

// Codes after 0xE0
static uint8_t extended_to_hid(uint8_t code) {
    switch (code) {
    case 0x1C: return 0x58; // keypad Enter
    case 0x1D: return 0xE4; // right Ctrl
    case 0x35: return 0x54; // keypad /
    case 0x37: return 0x46; // Print Screen
    case 0x38: return 0xE6; // right Alt (AltGr)
    case 0x47: return 0x4A; // Home
    case 0x48: return 0x52; // Up
    case 0x49: return 0x4B; // Page Up
    case 0x4B: return 0x50; // Left
    case 0x4D: return 0x4F; // Right
    case 0x4F: return 0x4D; // End
    case 0x50: return 0x51; // Down
    case 0x51: return 0x4E; // Page Down
    case 0x52: return 0x49; // Insert
    case 0x53: return 0x4C; // Delete
    case 0x5B: return 0xE3; // left GUI
    case 0x5C: return 0xE7; // right GUI
    case 0x5D: return 0x65; // Menu
    default: return 0;      // incl. the fake shifts around Print Screen (0x2A)
    }
}

static int extended, pause_skip;
static uint8_t modifiers; // HID modifier byte

// One scancode to a key event { usage, modifiers, flags, 0 }; returns 0 if
// it makes none. The keyboard repeats held keys itself (flags bit 1).
static int decode(uint8_t sc, uint8_t ev[4]) {
    if (pause_skip) { pause_skip--; return 0; }
    if (sc == 0xE1) { pause_skip = 5; return 0; } // Pause: ignored
    if (sc == 0xE0) { extended = 1; return 0; }
    int ext = extended;
    extended = 0;
    int pressed = !(sc & 0x80);
    uint8_t code = sc & 0x7F;
    uint8_t usage = ext ? extended_to_hid(code) : code < sizeof set1_to_hid ? set1_to_hid[code] : 0;
    if (!usage) return 0;
    if (usage >= 0xE0 && usage <= 0xE7) {
        uint8_t bit = (uint8_t)(1u << (usage - 0xE0));
        if (pressed) modifiers |= bit; else modifiers &= (uint8_t)~bit;
    }
    ev[0] = usage;
    ev[1] = modifiers;
    ev[2] = (uint8_t)(pressed | 2);
    ev[3] = 0;
    return 1;
}

// Read every byte the controller holds; returns the bytes of events made
static int drain(uint8_t *out, int max) {
    int n = 0;
    for (int i = 0; i < 64; i++) {
        uint8_t st = rx_inb(STATUS_PORT);
        if (!(st & STATUS_OBF)) break;
        uint8_t b = rx_inb(DATA_PORT);
        if (st & (STATUS_AUX | STATUS_ERR)) continue;
        if (n + 4 <= max && decode(b, out + n)) n += 4;
    }
    return n;
}

int main(void) {
    rx_handle_t res = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    rx_handle_t out = rx_take_startup_handle(RX_PA_USER0);
    rx_handle_t irq;
    uint8_t buf[256];

    if (rx_ioports_request(res, DATA_PORT, 1) != RX_OK || rx_ioports_request(res, STATUS_PORT, 1) != RX_OK) {
        rx_puts("ps2kbd: no access to the keyboard ports\n");
        rx_exit(3);
    }
    if (rx_inb(STATUS_PORT) == 0xFF) {
        rx_exit(2); // nothing answers: no 8042 controller
    }
    if (rx_interrupt_create(res, 1, 0, &irq) != RX_OK) {
        rx_puts("ps2kbd: cannot bind IRQ 1\n");
        rx_exit(3);
    }
    rx_handle_close(res);

    // Keys typed before we bound the IRQ are still in the controller
    int n = drain(buf, sizeof buf);
    if (n) rx_channel_write(out, 0, buf, n, 0, 0);

    for (;;) {
        if (rx_interrupt_wait(irq, 0) != RX_OK) rx_exit(3);
        n = drain(buf, sizeof buf);
        if (n && rx_channel_write(out, 0, buf, n, 0, 0) == RX_ERR_PEER_CLOSED) rx_exit(0);
    }
}
