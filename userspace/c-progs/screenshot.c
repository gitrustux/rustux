// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// screenshot FILE: what is on the screen now (the text console or the
// graphics), from the display service, as a PNG (8-bit RGB, stored without
// compression: big but simple). Root only (DESKTOP.md step F2). It also
// prints the CRC-32 of the pixels (RGB bytes, rows top to bottom, as in a
// PPM file), so a screenshot can be compared with QEMU's screendump.

#include <errno.h>
#include <fcntl.h>
#include <rustux/display.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static uint32_t crc_table[256];

static void crc_init(void) {
    for (uint32_t n = 0; n < 256; n++) {
        uint32_t c = n;
        for (int k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
        crc_table[n] = c;
    }
}

static uint32_t crc_update(uint32_t c, const uint8_t *p, size_t n) {
    for (size_t i = 0; i < n; i++) c = crc_table[(c ^ p[i]) & 0xFF] ^ (c >> 8);
    return c;
}

static int fd;
static int failed;

static void out(const void *p, size_t n) {
    const char *b = p;
    while (n && !failed) {
        ssize_t w = write(fd, b, n);
        if (w <= 0) {
            failed = 1;
            return;
        }
        b += w;
        n -= (size_t)w;
    }
}

static void be32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

// A chunk whose data is written in pieces: start, data..., end
static uint32_t chunk_crc;
static void chunk_start(const char *type, uint32_t len) {
    uint8_t h[8];
    be32(h, len);
    memcpy(h + 4, type, 4);
    out(h, 8);
    chunk_crc = crc_update(0xFFFFFFFFu, h + 4, 4);
}
static void chunk_data(const void *p, size_t n) {
    out(p, n);
    chunk_crc = crc_update(chunk_crc, p, n);
}
static void chunk_end(void) {
    uint8_t c[4];
    be32(c, chunk_crc ^ 0xFFFFFFFFu);
    out(c, 4);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        fprintf(stderr, "usage: screenshot FILE.png\n");
        return 2;
    }
    uint32_t *px, w, h;
    int r = rx_display_snapshot(&px, &w, &h);
    if (r < 0) {
        fprintf(stderr, "screenshot: %s\n", r == -ENOENT ? "no display service" : strerror(-r));
        return 1;
    }
    fd = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        fprintf(stderr, "screenshot: %s: %s\n", argv[1], strerror(errno));
        return 1;
    }
    crc_init();
    static const uint8_t sig[8] = { 0x89, 'P', 'N', 'G', '\r', '\n', 0x1A, '\n' };
    out(sig, 8);
    uint8_t ihdr[13];
    be32(ihdr, w);
    be32(ihdr + 4, h);
    ihdr[8] = 8;    // bits per sample
    ihdr[9] = 2;    // RGB
    ihdr[10] = ihdr[11] = ihdr[12] = 0;
    chunk_start("IHDR", 13);
    chunk_data(ihdr, 13);
    chunk_end();
    // The image data: zlib, with stored deflate blocks of whole rows (a
    // row is 1 filter byte and 3 bytes a pixel; rows over 64 KiB, wider
    // than 21845 pixels, are not supported)
    uint32_t row = 1 + 3 * w;
    if (row > 65535) {
        fprintf(stderr, "screenshot: the screen is too wide\n");
        return 1;
    }
    uint32_t per_block = 65535 / row, nblocks = (h + per_block - 1) / per_block;
    uint32_t len = 2 + nblocks * 5 + row * h + 4;
    chunk_start("IDAT", len);
    static const uint8_t zhdr[2] = { 0x78, 0x01 };
    chunk_data(zhdr, 2);
    uint32_t a = 1, b = 0;  // Adler-32
    uint32_t pcrc = 0xFFFFFFFFu;
    uint8_t *buf = malloc(row);
    for (uint32_t y = 0; y < h; y += per_block) {
        uint32_t rows = h - y < per_block ? h - y : per_block, n = rows * row;
        uint8_t bh[5] = { (uint8_t)(y + rows >= h), (uint8_t)n, (uint8_t)(n >> 8), (uint8_t)~n, (uint8_t)(~n >> 8) };
        chunk_data(bh, 5);
        for (uint32_t k = 0; k < rows; k++) {
            const uint32_t *src = px + (uint64_t)(y + k) * w;
            buf[0] = 0;
            for (uint32_t x = 0; x < w; x++) {
                buf[1 + 3 * x] = (uint8_t)(src[x] >> 16);
                buf[2 + 3 * x] = (uint8_t)(src[x] >> 8);
                buf[3 + 3 * x] = (uint8_t)src[x];
            }
            pcrc = crc_update(pcrc, buf + 1, row - 1);
            for (uint32_t i = 0; i < row; i++) {
                a = (a + buf[i]) % 65521;
                b = (b + a) % 65521;
            }
            chunk_data(buf, row);
        }
    }
    uint8_t ad[4];
    be32(ad, b << 16 | a);
    chunk_data(ad, 4);
    chunk_end();
    chunk_start("IEND", 0);
    chunk_end();
    if (close(fd) < 0) failed = 1;
    if (failed) {
        fprintf(stderr, "screenshot: cannot write %s\n", argv[1]);
        return 1;
    }
    printf("screenshot: %ux%u to %s, pixels crc32 %08x\n", w, h, argv[1], pcrc ^ 0xFFFFFFFFu);
    return 0;
}
