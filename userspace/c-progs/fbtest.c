// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// fbtest: draws through the display service (DESKTOP.md step F2): colour
// bars with rectangles moving over them, presenting only what changed,
// and reports the frame times. Esc or q ends it early; Ctrl+Alt+F1 shows
// the text console meanwhile (fbtest keeps going, unseen), Ctrl+Alt+F7
// brings it back.
//
//   fbtest [-n FRAMES]   animate (300 frames by default)
//   fbtest --pattern S   draw the test pattern and keep it S seconds
//                        (for comparing screenshots), then end
//   fbtest --check       draw the test pattern, take a snapshot through
//                        the service and compare: "fbtest: snapshot
//                        matches" (exit 0) or the first difference
//
// Needs root (the display service gives the screen to root only, until
// the compositor).

#include <rustux/display.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static rx_display_t d;

static const uint32_t bars[8] = { 0xF8F8F2, 0xF1FA8C, 0x8BE9FD, 0x50FA7B, 0xFF79C6, 0xFF5555, 0xBD93F9, 0x282A36 };

static uint64_t now_us(void) { return rx_clock_get_monotonic() / 1000; }

// The background at (x, y): eight vertical bars over a grey ramp at the bottom
static uint32_t background(uint32_t x, uint32_t y) {
    if (y >= d.height - d.height / 8) {
        uint32_t v = (uint32_t)((uint64_t)x * 255 / (d.width - 1));
        return v << 16 | v << 8 | v;
    }
    return bars[(uint64_t)x * 8 / d.width];
}

static void fill_background(uint32_t x0, uint32_t y0, uint32_t w, uint32_t h) {
    for (uint32_t y = y0; y < y0 + h && y < d.height; y++)
        for (uint32_t x = x0; x < x0 + w && x < d.width; x++) d.pixels[(uint64_t)y * d.width + x] = background(x, y);
}

static void fill(uint32_t x0, uint32_t y0, uint32_t w, uint32_t h, uint32_t c) {
    for (uint32_t y = y0; y < y0 + h && y < d.height; y++)
        for (uint32_t x = x0; x < x0 + w && x < d.width; x++) d.pixels[(uint64_t)y * d.width + x] = c;
}

// The fixed test pattern: the background, a white frame, a black square
// in the middle with a one-pixel checkerboard in it
static void pattern(void) {
    fill_background(0, 0, d.width, d.height);
    fill(0, 0, d.width, 2, 0xFFFFFF);
    fill(0, d.height - 2, d.width, 2, 0xFFFFFF);
    fill(0, 0, 2, d.height, 0xFFFFFF);
    fill(d.width - 2, 0, 2, d.height, 0xFFFFFF);
    uint32_t s = d.height / 4, x0 = d.width / 2 - s / 2, y0 = d.height / 2 - s / 2;
    for (uint32_t y = 0; y < s; y++)
        for (uint32_t x = 0; x < s; x++)
            d.pixels[(uint64_t)(y0 + y) * d.width + x0 + x] = (x + y) & 1 ? 0x000000 : 0xFF0000 >> ((x / 16) % 3 * 8);
}

// Esc or q pressed, or the display service gone
static int quit_key(void) {
    rx_display_msg_t ev;
    int r;
    while ((r = rx_display_poll(&d, &ev)) != 0) {
        if (r < 0) return 1;
        if (ev.op != RX_DISPLAY_EV_KEY || !((ev.a >> 16) & 1)) continue;
        uint8_t usage = ev.a & 0xFF;
        if (usage == 0x29 || usage == 0x14) return 1; // Esc, q
    }
    return 0;
}

static int animate(int frames) {
    enum { N = 6 };
    struct { int x, y, dx, dy; uint32_t w, h, c; } r[N];
    for (int i = 0; i < N; i++) {
        r[i].w = d.width / 10 + i * 8;
        r[i].h = d.height / 10 + i * 6;
        r[i].x = (int)((d.width - r[i].w) * (uint64_t)(i + 1) / (N + 1));
        r[i].y = (int)((d.height - r[i].h) * (uint64_t)(N - i) / (N + 1));
        r[i].dx = (i & 1 ? -1 : 1) * (4 + i * 2);
        r[i].dy = (i & 2 ? -1 : 1) * (3 + i);
        r[i].c = bars[(i + 3) % 7] ^ 0x202020;
    }
    fill_background(0, 0, d.width, d.height);
    rx_display_present(&d, 0, 0);
    uint64_t t0 = now_us(), draw_total = 0, present_total = 0, worst = 0;
    int done = 0, shown_frames = 0;
    for (; done < frames; done++) {
        if (quit_key()) break;
        uint64_t f0 = now_us();
        rx_display_rect_t dmg[2 * N];
        int nd = 0;
        for (int i = 0; i < N; i++) {
            dmg[nd++] = (rx_display_rect_t){ (uint32_t)r[i].x, (uint32_t)r[i].y, r[i].w, r[i].h };
            fill_background((uint32_t)r[i].x, (uint32_t)r[i].y, r[i].w, r[i].h);
        }
        for (int i = 0; i < N; i++) {
            r[i].x += r[i].dx;
            r[i].y += r[i].dy;
            if (r[i].x < 0 || r[i].x + (int)r[i].w > (int)d.width) {
                r[i].dx = -r[i].dx;
                r[i].x += 2 * r[i].dx;
            }
            if (r[i].y < 0 || r[i].y + (int)r[i].h > (int)d.height) {
                r[i].dy = -r[i].dy;
                r[i].y += 2 * r[i].dy;
            }
            fill((uint32_t)r[i].x, (uint32_t)r[i].y, r[i].w, r[i].h, r[i].c);
            dmg[nd++] = (rx_display_rect_t){ (uint32_t)r[i].x, (uint32_t)r[i].y, r[i].w, r[i].h };
        }
        uint64_t f1 = now_us();
        int vis = rx_display_present(&d, dmg, nd);
        uint64_t f2 = now_us();
        if (vis < 0) {
            fprintf(stderr, "fbtest: the display service went away\n");
            return 1;
        }
        draw_total += f1 - f0;
        present_total += f2 - f1;
        if (f2 - f0 > worst) worst = f2 - f0;
        if (vis) shown_frames++;
        else usleep(16000); // unseen (text console): no need to hurry
    }
    uint64_t total = now_us() - t0;
    if (!done) done = 1;
    printf("fbtest: %ux%u, %d frames in %llu ms (%llu frames/s), %d shown\n", d.width, d.height, done,
           (unsigned long long)total / 1000, (unsigned long long)(done * 1000000ull / (total ? total : 1)), shown_frames);
    printf("fbtest: per frame %llu us drawing, %llu us presenting, %llu us at worst\n",
           (unsigned long long)(draw_total / done), (unsigned long long)(present_total / done),
           (unsigned long long)worst);
    return 0;
}

static int check(void) {
    pattern();
    if (rx_display_present(&d, 0, 0) != 1) {
        printf("fbtest: the graphics are not on the screen\n");
        return 1;
    }
    uint32_t *snap, w, h;
    int r = rx_display_snapshot(&snap, &w, &h);
    if (r < 0) {
        printf("fbtest: no snapshot (%d)\n", r);
        return 1;
    }
    if (w != d.width || h != d.height) {
        printf("fbtest: snapshot is %ux%u, the screen %ux%u\n", w, h, d.width, d.height);
        return 1;
    }
    for (uint64_t i = 0; i < (uint64_t)w * h; i++) {
        if ((snap[i] & 0xFFFFFF) != (d.pixels[i] & 0xFFFFFF)) {
            printf("fbtest: snapshot differs at %llu,%llu: %06x, drawn %06x\n", (unsigned long long)(i % w),
                   (unsigned long long)(i / w), snap[i], d.pixels[i]);
            return 1;
        }
    }
    free(snap);
    printf("fbtest: snapshot matches (%ux%u)\n", w, h);
    return 0;
}

int main(int argc, char **argv) {
    int frames = 300, hold = -1, do_check = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "-n") && i + 1 < argc) frames = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--pattern") && i + 1 < argc) hold = atoi(argv[++i]);
        else if (!strcmp(argv[i], "--check")) do_check = 1;
        else {
            fprintf(stderr, "usage: fbtest [-n FRAMES] [--pattern SECONDS] [--check]\n");
            return 2;
        }
    }
    int r = rx_display_open(&d);
    if (r < 0) {
        fprintf(stderr, "fbtest: no display: %s\n", strerror(-r));
        return 1;
    }
    if (do_check) r = check();
    else if (hold >= 0) {
        pattern();
        rx_display_present(&d, 0, 0);
        for (int i = 0; i < hold * 10 && !quit_key(); i++) usleep(100000);
        r = 0;
    } else r = animate(frames);
    rx_display_close(&d);
    return r;
}
