// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// ps2kbd: the PS/2 keyboard driver, as an ordinary user program.
//
// The input service (inputd, DESKTOP.md step F3) starts it with the root
// resource and a channel. It asks the kernel
// for the keyboard's two I/O ports (0x60 data, 0x64 status) and for IRQ 1,
// then loops: wait for the interrupt, read the scancodes, turn them into key
// events (HID usages; inputd applies the layout), send them over the
// channel. The kernel only set up the 8042
// controller at boot; from here on it never touches the keyboard.
//
// A mouse on the auxiliary port (QEMU's default mouse, some touchpads in
// PS/2 mode; DESKTOP.md step F3) is set up too, with the wheel if it is an
// IntelliMouse, and read on IRQ 12 by a second thread: its packets become
// pointer events (8 bytes: buttons, wheel, flags 0, 0x81, then dx and dy
// as little-endian 16-bit numbers, y down), as the kernel's USB mice send.
//
// Exit codes: 0 channel closed, 2 no PS/2 controller (init does not
// restart it), 3 setup failed.

#include <pthread.h>
#include <unistd.h>
#include <rustux/process.h>
#include "rx.h"

#define DATA_PORT   0x60
#define STATUS_PORT 0x64
#define STATUS_OBF  0x01    // output buffer full: a byte is waiting
#define STATUS_AUX  0x20    // the byte is from the mouse
#define STATUS_ERR  0xC0    // parity / timeout error

// Scancode set 1 (what the 8042 delivers with translation on) to HID
// usages (page 0x07), the codes inputd's keyboard layout works with
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

// ---- the mouse ----

static int mouse;           // set up: 3 bytes a packet, or 4 with a wheel
static uint8_t packet[4];
static int got;

// One byte from the mouse; 8 bytes of event when a packet is complete
static int mouse_byte(uint8_t b, uint8_t *out) {
    if (!mouse) return 0;
    if (got == 0 && !(b & 0x08)) return 0; // not a first byte: resynchronise
    packet[got++] = b;
    if (got < mouse) return 0;
    got = 0;
    uint8_t f = packet[0];
    if (f & 0xC0) return 0; // overflow
    int dx = packet[1] - ((f & 0x10) ? 256 : 0);
    int dy = packet[2] - ((f & 0x20) ? 256 : 0);
    dy = -dy;               // the mouse counts up, the screen down
    int wheel = mouse == 4 ? (int8_t)(packet[3] << 4) >> 4 : 0;
    // A wheel step up is +1 on the wheel axis, as USB reports it
    wheel = -wheel;
    out[0] = f & 7;
    out[1] = (uint8_t)(int8_t)wheel;
    out[2] = 0;
    out[3] = 0x81;
    out[4] = (uint8_t)dx;
    out[5] = (uint8_t)((uint16_t)dx >> 8);
    out[6] = (uint8_t)dy;
    out[7] = (uint8_t)((uint16_t)dy >> 8);
    return 8;
}

static int wait_in(void) {  // the controller can take a byte
    for (int i = 0; i < 100000; i++)
        if (!(rx_inb(STATUS_PORT) & 2)) return 1;
    return 0;
}

static int read_byte(uint8_t *b) {
    uint64_t end = rx_clock_get_monotonic() + 50000000ull; // 50 ms
    while (rx_clock_get_monotonic() < end) {
        if (rx_inb(STATUS_PORT) & STATUS_OBF) {
            *b = rx_inb(DATA_PORT);
            return 1;
        }
    }
    return 0;
}

static int ctrl_cmd(uint8_t c) {
    if (!wait_in()) return 0;
    rx_outb(STATUS_PORT, c);
    return 1;
}

// A command to the mouse; 1 if it acknowledged (0xFA)
static int aux_cmd(uint8_t c) {
    uint8_t b = 0;
    if (!ctrl_cmd(0xD4) || !wait_in()) return 0;
    rx_outb(DATA_PORT, c);
    return read_byte(&b) && b == 0xFA;
}

// Enable the auxiliary port and set the mouse up; 0, 3 or 4 bytes a packet
static int mouse_setup(void) {
    uint8_t cfg = 0, b;
    if (!ctrl_cmd(0xA8)) return 0;              // enable the auxiliary port
    if (!ctrl_cmd(0x20) || !read_byte(&cfg)) return 0;
    if (!aux_cmd(0xF6)) return 0;               // defaults (also: is anyone there?)
    int size = 3;
    // IntelliMouse: sample rates 200, 100, 80, then the ID says 3
    if (aux_cmd(0xF3) && aux_cmd(200) && aux_cmd(0xF3) && aux_cmd(100) && aux_cmd(0xF3) && aux_cmd(80) &&
        aux_cmd(0xF2) && read_byte(&b) && b == 3)
        size = 4;
    aux_cmd(0xF3);
    aux_cmd(100);                               // 100 samples a second
    if (!aux_cmd(0xF4)) return 0;               // report
    cfg = (uint8_t)((cfg | 0x02) & ~0x20);      // IRQ 12 on, the mouse's clock on
    if (!ctrl_cmd(0x60) || !wait_in()) return 0;
    rx_outb(DATA_PORT, cfg);
    return size;
}

// ---- reading ----

static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static rx_handle_t out;

// Read every byte the controller holds (either interrupt) and send the
// events made
static int drain_and_send(void) {
    uint8_t buf[256];
    int n = 0;
    pthread_mutex_lock(&lock);
    for (int i = 0; i < 64; i++) {
        uint8_t st = rx_inb(STATUS_PORT);
        if (!(st & STATUS_OBF)) break;
        uint8_t b = rx_inb(DATA_PORT);
        if (st & STATUS_ERR) continue;
        if (st & STATUS_AUX) {
            if (n + 8 <= (int)sizeof buf) n += mouse_byte(b, buf + n);
        } else if (n + 4 <= (int)sizeof buf && decode(b, buf + n)) {
            n += 4;
        }
    }
    pthread_mutex_unlock(&lock);
    if (n && rx_channel_write(out, 0, buf, n, 0, 0) == RX_ERR_PEER_CLOSED) return -1;
    return n;
}

// The input service is gone (it was killed or ended): end too, freeing
// the IRQs for the driver its successor starts
static void *watch_thread(void *arg) {
    (void)arg;
    uint32_t pend = 0;
    rx_object_wait_one(out, RX_PEER_CLOSED, RX_TIME_INFINITE, &pend);
    rx_exit(0);
    return 0;
}

static void *mouse_thread(void *arg) {
    rx_handle_t irq12 = (rx_handle_t)(uintptr_t)arg;
    for (;;) {
        if (rx_interrupt_wait(irq12, 0) != RX_OK) return 0;
        if (drain_and_send() < 0) rx_exit(0);
    }
}

int main(void) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    rx_handle_t res = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    out = rx_take_startup_handle(RX_PA_USER0);
    rx_handle_t irq, irq12 = 0;

    if (rx_ioports_request(res, DATA_PORT, 1) != RX_OK || rx_ioports_request(res, STATUS_PORT, 1) != RX_OK) {
        rx_puts("ps2kbd: no access to the keyboard ports\n");
        rx_exit(3);
    }
    if (rx_inb(STATUS_PORT) == 0xFF) {
        rx_exit(2); // nothing answers: no 8042 controller
    }
    // A driver of the input service that just ended may still hold the
    // IRQs for a moment (it ends when it sees its channel close)
    rx_status_t bs = RX_ERR_BAD_STATE;
    for (int tries = 0; tries < 20; tries++) {
        if ((bs = rx_interrupt_create(res, 1, 0, &irq)) == RX_OK) break;
        usleep(50000);
    }
    if (bs != RX_OK) {
        rx_puts("ps2kbd: cannot bind IRQ 1\n");
        rx_exit(3);
    }
    // The mouse, if there is one (nothing is lost if not: its absence
    // just times out)
    rx_status_t ms = RX_ERR_BAD_STATE;
    for (int tries = 0; tries < 10 && ms != RX_OK; tries++)
        if ((ms = rx_interrupt_create(res, 12, 0, &irq12)) != RX_OK) usleep(50000);
    if (ms == RX_OK) {
        mouse = mouse_setup();
        pthread_t t;
        if (!mouse || pthread_create(&t, 0, mouse_thread, (void *)(uintptr_t)irq12) != 0) {
            mouse = 0;
            rx_handle_close(irq12);
        }
    }
    rx_handle_close(res);

    pthread_t w;
    pthread_create(&w, 0, watch_thread, 0);

    // Keys typed before we bound the IRQ are still in the controller
    if (drain_and_send() < 0) rx_exit(0);

    for (;;) {
        if (rx_interrupt_wait(irq, 0) != RX_OK) rx_exit(3);
        if (drain_and_send() < 0) rx_exit(0);
    }
}
