#!/usr/bin/env python3
# Copyright 2026 The Rustux Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT
"""Turn a picture into a wallpaper for files/usr/share/backgrounds: QOI
(qoiformat.org), the format the compositor reads.

  scripts/make-wallpaper.py IN OUT.qoi [--crop X,Y,W,H] [--max-width N]

--crop cuts a part out first (a portrait version of a landscape picture);
--max-width scales it down if it is wider (Lanczos). Needs Pillow."""

import argparse
import struct
import sys

from PIL import Image


def qoi_encode(img):
    w, h = img.size
    px = img.convert("RGB").tobytes()
    out = bytearray(b"qoif" + struct.pack(">IIBB", w, h, 3, 0))
    index = [(0, 0, 0, 0)] * 64
    prev = (0, 0, 0, 255)
    run = 0
    n = w * h
    for i in range(n):
        p = (px[3 * i], px[3 * i + 1], px[3 * i + 2], 255)
        if p == prev:
            run += 1
            if run == 62 or i == n - 1:
                out.append(0xC0 | (run - 1))
                run = 0
            continue
        if run:
            out.append(0xC0 | (run - 1))
            run = 0
        k = (p[0] * 3 + p[1] * 5 + p[2] * 7 + p[3] * 11) % 64
        if index[k] == p:
            out.append(k)
        else:
            index[k] = p
            dr = (p[0] - prev[0] + 128) % 256 - 128
            dg = (p[1] - prev[1] + 128) % 256 - 128
            db = (p[2] - prev[2] + 128) % 256 - 128
            if -2 <= dr <= 1 and -2 <= dg <= 1 and -2 <= db <= 1:
                out.append(0x40 | (dr + 2) << 4 | (dg + 2) << 2 | (db + 2))
            elif -32 <= dg <= 31 and -8 <= dr - dg <= 7 and -8 <= db - dg <= 7:
                out.append(0x80 | (dg + 32))
                out.append((dr - dg + 8) << 4 | (db - dg + 8))
            else:
                out += bytes([0xFE, p[0], p[1], p[2]])
        prev = p
    out += b"\x00" * 7 + b"\x01"
    return bytes(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--crop")
    ap.add_argument("--max-width", type=int)
    a = ap.parse_args()
    img = Image.open(a.src).convert("RGB")
    if a.crop:
        x, y, w, h = map(int, a.crop.split(","))
        img = img.crop((x, y, x + w, y + h))
    if a.max_width and img.width > a.max_width:
        img = img.resize((a.max_width, round(img.height * a.max_width / img.width)), Image.LANCZOS)
    data = qoi_encode(img)
    open(a.dst, "wb").write(data)
    print(f"{a.dst}: {img.width}x{img.height}, {len(data)} bytes")


if __name__ == "__main__":
    sys.exit(main())
