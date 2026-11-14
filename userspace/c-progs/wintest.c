// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// wintest: a window for testing the compositor (DESKTOP.md step F4).
// Fills its window with a colour, its name and a frame counter, and
// prints every event it gets ("wintest NAME: ..."), so the tests can see
// who received what. It answers size changes, closes when asked, and
// can be told not to.
//
//   wintest NAME [-c RRGGBB] [-s WxH] [-p X,Y] [-a] [-n] [-k] [-t TEXT]
//     -c   the colour (default from the name)
//     -s   the size (320x200)
//     -p   where the content goes on the screen (else the compositor chooses)
//     -a   animate: redraw every frame (paced by FRAME events)
//     -n   ignore close requests (the compositor removes it after 5 s)
//     -k   on key c: copy TEXT (-t) to the clipboard; on key v: paste
//          and print it
//
// Typed characters show in the window's last line.

#include <rustux/font8x16.h>
#include <rustux/window.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static rx_window_t win;
static const char *name;
static uint32_t color;
static char typed[64];
static unsigned frame;

static void text(int x, int y, const char *s, uint32_t c) {
    for (; *s; s++, x += 8) {
        const uint8_t *g = font8x16_glyph((unsigned char)*s);
        for (int r = 0; r < 16; r++)
            for (int b = 0; b < 8; b++)
                if ((g[r] & (0x80 >> b)) && x + b >= 0 && x + b < (int)win.width && y + r >= 0 && y + r < (int)win.height)
                    win.pixels[(uint64_t)(y + r) * win.width + x + b] = c;
    }
}

static void draw(void) {
    for (uint64_t i = 0; i < (uint64_t)win.width * win.height; i++) win.pixels[i] = color;
    char line[96];
    snprintf(line, sizeof line, "%s %ux%u", name, win.width, win.height);
    text(8, 8, line, 0x000000);
    snprintf(line, sizeof line, "frame %u", frame);
    text(8, 28, line, 0x000000);
    text(8, (int)win.height - 24, typed, 0x000000);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: wintest NAME [-c RRGGBB] [-s WxH] [-p X,Y] [-a] [-n] [-k] [-t TEXT]\n");
        return 2;
    }
    name = argv[1];
    unsigned w = 320, h = 200;
    int x = -1, y = -1, animate = 0, stubborn = 0, clip = 0;
    const char *copy_text = "copied text";
    color = 0x808080;
    for (const char *p = name; *p; p++) color = color * 31 + (unsigned char)*p;
    color = (color & 0x7F7F7F) | 0x808080;  // light
    for (int i = 2; i < argc; i++) {
        if (!strcmp(argv[i], "-c") && i + 1 < argc) color = (uint32_t)strtoul(argv[++i], 0, 16);
        else if (!strcmp(argv[i], "-s") && i + 1 < argc) {
            char *e;
            w = (unsigned)strtoul(argv[++i], &e, 10);
            h = *e ? (unsigned)strtoul(e + 1, 0, 10) : w;
        } else if (!strcmp(argv[i], "-p") && i + 1 < argc) {
            char *e;
            x = (int)strtol(argv[++i], &e, 10);
            y = *e ? (int)strtol(e + 1, 0, 10) : x;
        }
        else if (!strcmp(argv[i], "-a")) animate = 1;
        else if (!strcmp(argv[i], "-n")) stubborn = 1;
        else if (!strcmp(argv[i], "-k")) clip = 1;
        else if (!strcmp(argv[i], "-t") && i + 1 < argc) copy_text = argv[++i];
    }
    int r = rx_window_create(&win, x, y, w, h, name);
    if (r < 0) {
        fprintf(stderr, "wintest %s: no window: %s\n", name, strerror(-r));
        return 1;
    }
    printf("wintest %s: window %ux%u\n", name, win.width, win.height);
    fflush(stdout);
    draw();
    rx_window_commit(&win, 0, 0);
    for (;;) {
        rx_window_event_t ev;
        uint64_t deadline = RX_TIME_INFINITE;
        if (animate && !win.frame_pending) {
            if (++frame % 100 == 0) {
                printf("wintest %s: %u frames\n", name, frame);
                fflush(stdout);
            }
            draw();
            rx_window_commit(&win, 0, 0);
        }
        r = rx_window_next_event(&win, &ev, animate ? rx_clock_get_monotonic() + 100000000ull : deadline);
        if (r < 0) {
            printf("wintest %s: the compositor is gone\n", name);
            return 1;
        }
        if (r == 0) continue;
        switch (ev.op) {
        case RX_WINDOW_EV_FRAME:
            break;
        case RX_WINDOW_EV_CONFIGURE:
            printf("wintest %s: configure %ux%u\n", name, ev.w, ev.h);
            if (rx_window_resize(&win, ev.w, ev.h) == 0) {
                draw();
                rx_window_commit(&win, 0, 0);
            }
            break;
        case RX_WINDOW_EV_KEY:
            if (!(ev.flags & 1)) break;
            printf("wintest %s: key 0x%02x text \"%s\"\n", name, ev.code, ev.text[0] == '\r' ? "\\r" : ev.text);
            if (clip && !strcmp(ev.text, "c")) {
                rx_window_clipboard_set(&win, copy_text);
                printf("wintest %s: copied \"%s\"\n", name, copy_text);
            } else if (clip && !strcmp(ev.text, "v")) {
                char buf[256];
                int n = rx_window_clipboard_get(&win, buf, sizeof buf);
                printf("wintest %s: pasted \"%s\" (%d bytes)\n", name, n >= 0 ? buf : "", n);
            } else if (ev.text[0] >= ' ' && strlen(typed) + strlen(ev.text) < sizeof typed - 1) {
                strcat(typed, ev.text);
                draw();
                rx_window_commit(&win, 0, 0);
            }
            break;
        case RX_WINDOW_EV_POINTER:
            if (ev.flags & RX_WINDOW_PTR_LEAVE) printf("wintest %s: pointer left\n", name);
            else if (ev.flags & RX_WINDOW_PTR_BUTTON) printf("wintest %s: buttons 0x%x at %d,%d\n", name, ev.buttons, ev.x, ev.y);
            else if (ev.flags & RX_WINDOW_PTR_WHEEL) printf("wintest %s: wheel %d at %d,%d\n", name, ev.wheel, ev.x, ev.y);
            break;
        case RX_WINDOW_EV_FOCUS:
            printf("wintest %s: focus %s\n", name, ev.flags ? "in" : "out");
            break;
        case RX_WINDOW_EV_STATE:
            printf("wintest %s: state 0x%x\n", name, ev.flags);
            break;
        case RX_WINDOW_EV_CLOSE_REQUEST:
            printf("wintest %s: close request%s\n", name, stubborn ? " (ignored)" : "");
            if (!stubborn) {
                rx_window_destroy(&win);
                printf("wintest %s: closed\n", name);
                return 0;
            }
            break;
        }
        fflush(stdout);
    }
}
