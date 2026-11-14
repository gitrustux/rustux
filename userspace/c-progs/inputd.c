// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// inputd: the input service (DESKTOP.md step F3, docs/INPUT.md,
// <rustux/input.h>).
//
// init starts it (and starts it again if it ends) with:
//   RX_PA_USER0          the kernel's input channel (USB keyboards, mice,
//                        tablets)
//   RX_PA_USER1          a channel to the text console (init): key
//                        presses with the bytes for the terminal
//   RX_PA_ROOT_RESOURCE  for /bin/ps2kbd, which inputd starts (and
//                        restarts) for the PS/2 keyboard and mouse
//
// Both sources send 4-byte key records (usage, modifiers, flags, 0) and
// 8-byte pointer records (buttons, wheel, flags, 0x81, x, y). inputd
// applies the layout (keymap.h, /etc/keymap), repeats held keys (USB; a
// PS/2 keyboard repeats by itself) and gives every event to the consumer
// with the focus: a program that grabbed the input, else the display's
// owner while graphics are shown, else the console. Ctrl+Alt+F1 and
// Ctrl+Alt+F7 switch between the console and graphics.

#include <errno.h>
#include <rustux/display.h>
#include <rustux/input.h>
#include <rustux/process.h>
#include <rustux/service.h>
#include <stdio.h>
#include <string.h>
#include "keymap.h"

#define MAX_CLIENTS 16

static rx_handle_t resource, kernel_in, console;
static rx_handle_t drv_proc, drv_chan;
static int driver_failures;

static rx_handle_t clients[MAX_CLIENTS];
static int nclients;
static rx_handle_t grabber;     // the client with the focus, or 0
static rx_handle_t gfx;         // the client for graphics (the compositor), or 0

static rx_handle_t display_ch;  // connected when first needed

static rx_handle_t repeat_timer;
static uint8_t repeat_usage, repeat_mods;
static int repeating;

// ---- the PS/2 driver ----

static void start_driver(void) {
    rx_handle_t ours, theirs, res;
    if (!resource || driver_failures >= 5) return;
    rx_channel_create(0, &ours, &theirs);
    rx_handle_duplicate(resource, RX_RIGHT_SAME_RIGHTS, &res);
    rx_spawn_handle_t hs[] = {
        { RX_PA_ROOT_RESOURCE, res },
        { RX_PA_USER0, theirs },
    };
    char *argv[] = { "ps2kbd", 0 };
    // Its stdout and stderr are ours (the console)
    if (rx_spawn("/bin/ps2kbd", argv, 0, RX_SPAWN_CLONE_STDIO, hs, 2, &drv_proc) != RX_OK) {
        printf("inputd: cannot start /bin/ps2kbd\n");
        rx_handle_close(ours);
        drv_proc = 0;
        driver_failures = 5;
        return;
    }
    drv_chan = ours;
}

static void driver_ended(void) {
    rx_info_process_t info = { 0, 0, 0, 0 };
    rx_object_get_info(drv_proc, RX_INFO_PROCESS, &info, sizeof info, 0, 0);
    rx_handle_close(drv_proc);
    rx_handle_close(drv_chan);
    drv_proc = drv_chan = 0;
    if (info.return_code == 2) {
        printf("inputd: no PS/2 controller; keyboard driver not needed\n");
        return;
    }
    printf("inputd: keyboard driver exited (code %lld); starting it again\n", (long long)info.return_code);
    if (info.return_code != RX_TASK_RETCODE_SYSCALL_KILL) driver_failures++;
    start_driver();
}

// ---- consumers ----

static void to_display(uint32_t op, uint32_t a, uint32_t b) {
    rx_display_msg_t m = { op, 0, a, b };
    for (int tries = 0; tries < 2; tries++) {
        if (!display_ch && rx_service_connect("/svc/display", &display_ch) < 0) {
            display_ch = 0;
            return;
        }
        if (rx_channel_write(display_ch, 0, &m, sizeof m, 0, 0) == RX_OK) return;
        rx_handle_close(display_ch); // the service was restarted: connect again
        display_ch = 0;
    }
}

static int graphics_shown(void) { return rx_display_console(0, RX_DISPLAY_QUERY) == RX_DISPLAY_GRAPHICS; }

static void to_grabber(const rx_input_event_t *e) {
    if (rx_channel_write(grabber, 0, e, sizeof *e, 0, 0) != RX_OK) grabber = 0; // gone: the next event goes elsewhere
}

// To the graphics client while graphics are shown: 1 if sent
static int to_gfx(const rx_input_event_t *e) {
    if (!gfx || !graphics_shown()) return 0;
    if (rx_channel_write(gfx, 0, e, sizeof *e, 0, 0) != RX_OK) {
        gfx = 0;
        return 0;
    }
    return 1;
}

static void key(uint8_t usage, uint8_t mods, uint8_t flags) {
    int pressed = flags & 1;
    // Ctrl+Alt+F1: the text console; Ctrl+Alt+F7: graphics (whoever has the focus)
    if ((mods & KM_CTRL) && (mods & (KM_LALT | KM_RALT)) && (usage == 0x3A || usage == 0x40)) {
        if (pressed && !(flags & RX_INPUT_REPEAT))
            to_display(RX_DISPLAY_OP_SWITCH, usage == 0x3A ? RX_DISPLAY_TEXT : RX_DISPLAY_GRAPHICS, 0);
        return;
    }
    char text[16];
    int n = pressed && usage < 0xE0 ? km_translate(usage, mods, 1, text) : 0;
    if (n < 0 || n > 11) n = 0;
    text[n] = 0;
    // Repeat held keys (not modifiers; not what the PS/2 keyboard repeats itself)
    if (!(flags & RX_INPUT_REPEAT)) {
        if (pressed && usage < 0xE0 && !(flags & 2)) {
            repeating = 1;
            repeat_usage = usage;
            repeat_mods = mods;
            rx_timer_set(repeat_timer, rx_clock_get_monotonic() + 500000000ull, 0);
        } else if (!pressed && usage == repeat_usage) {
            repeating = 0;
        } else if (pressed) {
            repeating = 0;
        }
    }
    rx_input_event_t e;
    memset(&e, 0, sizeof e);
    e.type = RX_INPUT_KEY;
    e.code = usage;
    e.mods = mods;
    e.flags = (flags & 1) | (flags & RX_INPUT_REPEAT);
    memcpy(e.text, text, (size_t)n + 1);
    e.time = rx_clock_get_monotonic();
    if (grabber) {
        to_grabber(&e);
        return;
    }
    if (to_gfx(&e)) return;
    if (graphics_shown()) {
        to_display(RX_DISPLAY_OP_KEY, usage | (uint32_t)mods << 8 | (uint32_t)(flags & 7) << 16, 0);
        return;
    }
    if (!pressed || usage >= 0xE0) return; // the console wants presses
    rx_input_console_t c;
    memset(&c, 0, sizeof c);
    c.usage = usage;
    c.mods = mods;
    c.flags = flags;
    c.len = (uint8_t)n;
    memcpy(c.text, text, (size_t)n);
    rx_channel_write(console, 0, &c, sizeof c, 0, 0);
}

static void repeat_fired(void) {
    key(repeat_usage, repeat_mods, 1 | RX_INPUT_REPEAT);
    rx_timer_set(repeat_timer, rx_clock_get_monotonic() + 33000000ull, 0);
}

static void pointer(const unsigned char *r) {
    int16_t x = (int16_t)(r[4] | r[5] << 8), y = (int16_t)(r[6] | r[7] << 8);
    {
        rx_input_event_t e;
        memset(&e, 0, sizeof e);
        e.type = RX_INPUT_POINTER;
        e.code = r[0];
        e.flags = r[2] & RX_INPUT_ABSOLUTE;
        if (e.flags & RX_INPUT_ABSOLUTE) {
            e.x = (uint16_t)x;
            e.y = (uint16_t)y;
        } else {
            e.x = x;
            e.y = y;
        }
        e.wheel = (int8_t)r[1];
        e.time = rx_clock_get_monotonic();
        if (grabber) {
            to_grabber(&e);
            return;
        }
        if (to_gfx(&e)) return;
    }
    // The console has no pointer
    if (graphics_shown())
        to_display(RX_DISPLAY_OP_POINTER, r[0] | (uint32_t)r[1] << 8 | (uint32_t)r[2] << 16,
                   r[4] | (uint32_t)r[5] << 8 | (uint32_t)r[6] << 16 | (uint32_t)r[7] << 24);
}

// Records from a source: keys 4 bytes, pointer events (fourth byte 0x81) 8
static void read_source(rx_handle_t from) {
    unsigned char buf[256];
    uint32_t n = 0;
    while (rx_channel_read(from, 0, buf, 0, sizeof buf, 0, &n, 0) == RX_OK) {
        for (uint32_t i = 0; i + 4 <= n;) {
            if (buf[i + 3] == 0x81) {
                if (i + 8 <= n) pointer(buf + i);
                i += 8;
            } else {
                key(buf[i], buf[i + 1], buf[i + 2] & 3);
                i += 4;
            }
        }
    }
}

// ---- requests ----

static void drop_client(int k) {
    if (clients[k] == grabber) grabber = 0;
    if (clients[k] == gfx) gfx = 0;
    rx_handle_close(clients[k]);
    clients[k] = clients[--nclients];
}

// 1 handled, 2 nothing waiting, 0 the client is gone
static int request(int k) {
    rx_handle_t ch = clients[k];
    rx_input_req_t r;
    rx_handle_t h = 0;
    uint32_t nb = 0, nh = 0;
    rx_sender_t who;
    rx_status_t s = rx_channel_read_creds(ch, &r, &h, sizeof r, 1, &nb, &nh, &who);
    if (s == RX_ERR_SHOULD_WAIT) return 2;
    if (s != RX_OK) return 0;
    if (nh) rx_handle_close(h);
    if (nb < sizeof r) return 1;
    int32_t st = 0;
    switch (r.op) {
    case RX_INPUT_OP_GRAB:
        if (who.euid != 0) st = -EPERM;
        else if (grabber && grabber != ch) st = -EBUSY;
        else {
            grabber = ch;
            repeating = 0;
        }
        break;
    case RX_INPUT_OP_UNGRAB:
        if (grabber == ch) grabber = 0;
        break;
    case RX_INPUT_OP_GRAPHICS:
        if (who.euid != 0) st = -EPERM;
        else if (gfx && gfx != ch) st = -EBUSY;
        else gfx = ch;
        break;
    case RX_INPUT_OP_RELOAD_KEYMAP:
        km_load();
        break;
    case RX_INPUT_OP_RESTART_KBD:
        if (who.euid != 0) st = -EPERM;
        else if (!drv_proc) st = -ENOENT;
        else {
            printf("inputd: killing the keyboard driver\n");
            rx_task_kill(drv_proc);
        }
        break;
    default:
        st = -ENOSYS;
    }
    rx_channel_write(ch, 0, &st, sizeof st, 0, 0);
    return 1;
}

int main(void) {
    rx_malloc_commit(1); // a system service: memory committed when mapped (step F1)
    kernel_in = rx_take_startup_handle(RX_PA_USER0);
    console = rx_take_startup_handle(RX_PA_USER1);
    resource = rx_take_startup_handle(RX_PA_ROOT_RESOURCE);
    if (!console) {
        fprintf(stderr, "inputd: start me from init\n");
        return 2;
    }
    rx_timer_create(0, 0, &repeat_timer);
    km_load();
    start_driver();
    rx_handle_t listen, theirs;
    if (rx_channel_create(0, &listen, &theirs) != RX_OK || rx_service_publish("/svc/input", theirs) < 0) {
        fprintf(stderr, "inputd: cannot publish /svc/input\n");
        listen = 0;
    }
    for (;;) {
        rx_wait_item_t items[6 + MAX_CLIENTS];
        int n = 0, i_rep = -1, i_kin = -1, i_drv = -1, i_dproc = -1, i_lis = -1;
        if (repeating) { i_rep = n; items[n++] = (rx_wait_item_t){ repeat_timer, RX_TIMER_SIGNALED, 0 }; }
        if (kernel_in) { i_kin = n; items[n++] = (rx_wait_item_t){ kernel_in, RX_CHANNEL_READABLE, 0 }; }
        if (drv_chan) { i_drv = n; items[n++] = (rx_wait_item_t){ drv_chan, RX_CHANNEL_READABLE, 0 }; }
        if (drv_proc) { i_dproc = n; items[n++] = (rx_wait_item_t){ drv_proc, RX_PROCESS_TERMINATED, 0 }; }
        if (listen) { i_lis = n; items[n++] = (rx_wait_item_t){ listen, RX_CHANNEL_READABLE, 0 }; }
        int i_cl = n;
        for (int k = 0; k < nclients; k++) items[n++] = (rx_wait_item_t){ clients[k], RX_CHANNEL_READABLE | RX_PEER_CLOSED, 0 };
        int i_con = n;
        items[n++] = (rx_wait_item_t){ console, RX_PEER_CLOSED, 0 };
        rx_object_wait_many(items, (uint64_t)n, RX_TIME_INFINITE);
        if (items[i_con].pending & RX_PEER_CLOSED) return 0; // init is gone
        if (i_rep >= 0 && repeating && (items[i_rep].pending & RX_TIMER_SIGNALED)) repeat_fired();
        if (i_kin >= 0 && (items[i_kin].pending & RX_CHANNEL_READABLE)) read_source(kernel_in);
        if (i_drv >= 0 && (items[i_drv].pending & RX_CHANNEL_READABLE)) read_source(drv_chan);
        if (i_dproc >= 0 && (items[i_dproc].pending & RX_PROCESS_TERMINATED)) driver_ended();
        for (int k = nclients - 1; k >= 0; k--) {
            if (!items[i_cl + k].pending) continue;
            int r = 1;
            for (int i = 0; i < 16 && r == 1; i++) r = request(k);
            if (!r) drop_client(k);
        }
        if (i_lis >= 0 && (items[i_lis].pending & RX_CHANNEL_READABLE)) {
            char buf[64];
            rx_handle_t h;
            uint32_t nb = 0, nh = 0;
            while (rx_channel_read(listen, 0, buf, &h, sizeof buf, 1, &nb, &nh) == RX_OK) {
                if (!nh) continue;
                if (nclients == MAX_CLIENTS) rx_handle_close(h);
                else clients[nclients++] = h;
            }
        }
    }
}
