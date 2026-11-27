// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// soundctl: sound from the console (phase S, docs/SOUND.md).
//
//   soundctl [status]              the volume, muting, the devices and their outputs
//   soundctl volume [N|+N|-N]      set (0-100) or change it
//   soundctl mute|unmute|toggle
//   soundctl test                  a short tone, left then right then both
//   soundctl tone HZ SECONDS [RATE [CHANNELS]]   a sine tone (RATE in Hz, 1 or 2 channels)
//   soundctl play FILE.wav         a WAV file (16-bit PCM, 1 or 2 channels, any rate)
//
// It talks to soundd (/svc/sound); `play` is also linked as `play`.

#include <errno.h>
#include <math.h>
#include <rustux/sound.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int usage(void) {
    fprintf(stderr, "usage: soundctl [status] | volume [N|+N|-N] | mute | unmute | toggle | test\n"
                    "       soundctl tone HZ SECONDS [RATE [CHANNELS]] | play FILE.wav\n");
    return 2;
}

static int control(const char *req) {
    char out[4096];
    if (rx_sound_control(req, out, sizeof out) < 0) {
        fprintf(stderr, "soundctl: the sound service is not running (svc status sound)\n");
        return 1;
    }
    if (!strncmp(out, "error: ", 7)) {
        fprintf(stderr, "soundctl: %s", out + 7);
        return 1;
    }
    // Everything but the final "ok"
    char *end = strstr(out, "ok\n");
    if (end && (end == out || end[-1] == '\n')) *end = 0;
    fputs(out, stdout);
    return 0;
}

// `seconds` of a sine at `hz` (amplitude 0.5), on channel `which` (0
// left, 1 right, 2 both) of a stream at rate and channels
static int tone(rx_sound_t *s, double hz, double seconds, int which) {
    enum { CHUNK = 1024 };
    int16_t buf[CHUNK * 2];
    size_t total = (size_t)(seconds * s->rate);
    double ph = 0, step = 2 * M_PI * hz / s->rate;
    for (size_t done = 0; done < total;) {
        size_t n = total - done < CHUNK ? total - done : CHUNK;
        for (size_t i = 0; i < n; i++) {
            int16_t v = (int16_t)(sin(ph) * 16383);
            ph += step;
            if (ph > 2 * M_PI) ph -= 2 * M_PI;
            if (s->channels == 1) buf[i] = v;
            else {
                buf[2 * i] = which == 1 ? 0 : v;
                buf[2 * i + 1] = which == 0 ? 0 : v;
            }
        }
        ssize_t w = rx_sound_write(s, buf, n);
        if (w < 0) return (int)w;
        done += n;
    }
    return 0;
}

static int open_stream(rx_sound_t *s, uint32_t rate, uint32_t ch) {
    int r = rx_sound_open(s, rate, ch);
    if (r < 0) fprintf(stderr, "soundctl: cannot play: %s\n", r == -ENOENT ? "the sound service is not running" : strerror(-r));
    return r;
}

static uint32_t le32(const uint8_t *p) { return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24; }
static uint16_t le16(const uint8_t *p) { return (uint16_t)(p[0] | p[1] << 8); }

static int play(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        fprintf(stderr, "soundctl: %s: %s\n", path, strerror(errno));
        return 1;
    }
    uint8_t h[12];
    if (fread(h, 1, 12, f) != 12 || memcmp(h, "RIFF", 4) || memcmp(h + 8, "WAVE", 4)) {
        fprintf(stderr, "soundctl: %s: not a WAV file\n", path);
        fclose(f);
        return 1;
    }
    uint32_t rate = 0, chans = 0, bits = 0, fmt = 0;
    for (;;) {
        uint8_t c[8];
        if (fread(c, 1, 8, f) != 8) {
            fprintf(stderr, "soundctl: %s: no sound in it\n", path);
            fclose(f);
            return 1;
        }
        uint32_t len = le32(c + 4);
        if (!memcmp(c, "fmt ", 4)) {
            uint8_t b[40];
            size_t k = len < sizeof b ? len : sizeof b;
            if (fread(b, 1, k, f) != k) break;
            fmt = le16(b);
            chans = le16(b + 2);
            rate = le32(b + 4);
            bits = le16(b + 14);
            if (fmt == 0xFFFE && k >= 26) fmt = le16(b + 24); // WAVE_FORMAT_EXTENSIBLE: its subformat
            fseek(f, (long)(len - k + (len & 1)), SEEK_CUR);
        } else if (!memcmp(c, "data", 4)) {
            if (fmt != 1 || bits != 16 || chans < 1 || chans > 2 || rate < 8000 || rate > 192000) {
                fprintf(stderr, "soundctl: %s: only 16-bit PCM in 1 or 2 channels (this is format %u, %u bits, %u channels, %u Hz)\n", path, fmt,
                        bits, chans, rate);
                fclose(f);
                return 1;
            }
            rx_sound_t s;
            if (open_stream(&s, rate, chans) < 0) {
                fclose(f);
                return 1;
            }
            static int16_t buf[8192];
            uint32_t left = len;
            while (left) {
                size_t want = left < sizeof buf ? left : sizeof buf;
                size_t got = fread(buf, 1, want, f);
                if (!got) break;
                got -= got % (2 * chans);
                if (rx_sound_write(&s, buf, got / (2 * chans)) < 0) break;
                left -= (uint32_t)got;
            }
            rx_sound_drain(&s);
            rx_sound_close(&s);
            fclose(f);
            return 0;
        } else {
            fseek(f, (long)(len + (len & 1)), SEEK_CUR);
        }
    }
    fclose(f);
    return 1;
}

int main(int argc, char **argv) {
    const char *prog = strrchr(argv[0], '/') ? strrchr(argv[0], '/') + 1 : argv[0];
    if (!strcmp(prog, "play")) {
        if (argc != 2) {
            fprintf(stderr, "usage: play FILE.wav\n");
            return 2;
        }
        return play(argv[1]);
    }
    if (argc == 1 || !strcmp(argv[1], "status")) return control("status");
    const char *c = argv[1];
    if (!strcmp(c, "-h") || !strcmp(c, "--help")) {
        usage();
        return 0;
    }
    if (!strcmp(c, "volume")) {
        if (argc == 2) return control("status");
        char req[64];
        snprintf(req, sizeof req, "volume %s", argv[2]);
        return control(req);
    }
    if (!strcmp(c, "mute") || !strcmp(c, "unmute") || !strcmp(c, "toggle")) return control(c);
    if (!strcmp(c, "play") && argc == 3) return play(argv[2]);
    if (!strcmp(c, "test")) {
        rx_sound_t s;
        if (open_stream(&s, RX_SOUND_RATE, 2) < 0) return 1;
        printf("left, right, both\n");
        fflush(stdout);
        int r = tone(&s, 523.25, 0.5, 0);
        if (!r) r = tone(&s, 659.25, 0.5, 1);
        if (!r) r = tone(&s, 783.99, 0.7, 2);
        rx_sound_drain(&s);
        rx_sound_close(&s);
        return r < 0;
    }
    if (!strcmp(c, "tone") && argc >= 4) {
        double hz = atof(argv[2]), secs = atof(argv[3]);
        uint32_t rate = argc > 4 ? (uint32_t)atoi(argv[4]) : RX_SOUND_RATE;
        uint32_t ch = argc > 5 ? (uint32_t)atoi(argv[5]) : 2;
        if (hz <= 0 || secs <= 0 || secs > 600) return usage();
        rx_sound_t s;
        if (open_stream(&s, rate, ch) < 0) return 1;
        int r = tone(&s, hz, secs, 2);
        rx_sound_drain(&s);
        rx_sound_close(&s);
        return r < 0;
    }
    return usage();
}
