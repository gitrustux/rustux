// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ps2kbd: the PS/2 keyboard driver, as an ordinary user program.
//
// init starts it with the root resource and a channel. It asks the kernel
// for the keyboard's two I/O ports (0x60 data, 0x64 status) and for IRQ 1,
// then loops: wait for the interrupt, read the scancodes, turn them into
// characters, send them over the channel. The kernel only set up the 8042
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

// Scancode set 1 (what the 8042 delivers with translation on)
static const char normal[0x3A] = {
    0, 27, '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=', 8, '\t',
    'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']', '\n', 0, 'a', 's',
    'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', '\'', '`', 0, '\\', 'z', 'x', 'c', 'v',
    'b', 'n', 'm', ',', '.', '/', 0, '*', 0, ' ',
};
static const char shifted[0x3A] = {
    0, 27, '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+', 8, '\t',
    'Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', 'O', 'P', '{', '}', '\n', 0, 'A', 'S',
    'D', 'F', 'G', 'H', 'J', 'K', 'L', ':', '"', '~', 0, '|', 'Z', 'X', 'C', 'V',
    'B', 'N', 'M', '<', '>', '?', 0, '*', 0, ' ',
};

static int shift_l, shift_r, ctrl, caps, extended;

// One scancode; returns a character or 0
static char decode(uint8_t sc) {
    if (sc == 0xE0) {
        extended = 1;
        return 0;
    }
    int ext = extended;
    extended = 0;
    int release = sc & 0x80;
    uint8_t code = sc & 0x7F;
    switch (code) {
    case 0x2A: if (!ext) shift_l = !release; return 0;
    case 0x36: shift_r = !release; return 0;
    case 0x1D: ctrl = !release; return 0;
    case 0x38: return 0; // alt
    case 0x3A: if (!release) caps = !caps; return 0;
    }
    if (release) return 0;
    if (ext) return code == 0x1C ? '\n' : 0; // keypad Enter; arrows etc. ignored
    if (code >= sizeof normal) return 0;
    char c = (shift_l || shift_r) ? shifted[code] : normal[code];
    if (caps && c >= 'a' && c <= 'z') c -= 32;
    else if (caps && c >= 'A' && c <= 'Z' && (shift_l || shift_r)) c += 32;
    if (ctrl && c >= 'a' && c <= 'z') c = c - 'a' + 1;
    if (ctrl && c >= 'A' && c <= 'Z') c = c - 'A' + 1;
    return c;
}

// Read every byte the controller holds; returns how many characters
static int drain(char *out, int max) {
    int n = 0;
    for (int i = 0; i < 64; i++) {
        uint8_t st = rx_inb(STATUS_PORT);
        if (!(st & STATUS_OBF)) break;
        uint8_t b = rx_inb(DATA_PORT);
        if (st & (STATUS_AUX | STATUS_ERR)) continue;
        char c = decode(b);
        if (c && n < max) out[n++] = c;
    }
    return n;
}

int main(void) {
    rx_handle_t res = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    rx_handle_t out = rx_take_startup_handle(RX_PA_USER0);
    rx_handle_t irq;
    char buf[64];

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
