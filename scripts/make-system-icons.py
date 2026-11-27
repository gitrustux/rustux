#!/usr/bin/env python3
# The system's programs' icons (2026-11-27), in the style of the apps'
# (a rounded tile in a colour of its own, a white symbol): 64x64 PNG in
# files/usr/share/icons/rustica/, named after the .app files. Drawn at 4x
# and scaled down, so the edges are smooth. Needs Pillow.
#
#   scripts/make-system-icons.py [OUTDIR]
import math
import os
import shutil
import sys

from PIL import Image, ImageDraw

S = 256            # drawn at this size, saved at 64
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), '..', 'files', 'usr', 'share', 'icons', 'rustica')
WHITE = (255, 255, 255, 255)


def tile(colour):
    im = Image.new('RGBA', (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((8, 8, S - 8, S - 8), radius=52, fill=colour)
    return im, d


def save(im, name):
    im.resize((64, 64), Image.LANCZOS).save(os.path.join(OUT, name + '.png'))


def terminal():
    im, d = tile((32, 32, 36, 255))
    # ">" and "_" in the brand's orange
    o = (255, 101, 0, 255)
    d.line([(64, 84), (116, 128), (64, 172)], fill=o, width=22, joint='curve')
    d.line([(132, 176), (192, 176)], fill=o, width=22)
    save(im, 'terminal')


def files():
    im, d = tile((52, 120, 217, 255))
    # A folder: its tab, then its body
    d.rounded_rectangle((52, 76, 128, 116), radius=14, fill=(255, 196, 80, 255))
    d.rounded_rectangle((52, 96, 204, 184), radius=16, fill=(255, 210, 110, 255))
    save(im, 'files')


def settings():
    im, d = tile((90, 98, 112, 255))
    c, r_out, r_in, teeth = S / 2, 78, 58, 8
    pts = []
    for i in range(teeth * 4):
        a = i * math.pi * 2 / (teeth * 4)
        r = r_out if (i % 4) in (0, 1) else r_in
        pts.append((c + r * math.cos(a), c + r * math.sin(a)))
    d.polygon(pts, fill=WHITE)
    d.ellipse((c - 30, c - 30, c + 30, c + 30), fill=(90, 98, 112, 255))
    save(im, 'settings')


def window_icon(name, colour):
    # The demos: a window with a title bar
    im, d = tile(colour)
    d.rounded_rectangle((52, 64, 204, 192), radius=14, fill=WHITE)
    d.rectangle((52, 92, 204, 100), fill=colour)
    save(im, name)


def animation():
    im, d = tile((128, 80, 200, 255))
    for i, x in enumerate((70, 116, 162)):
        a = 90 + i * 70
        d.ellipse((x - 22, 128 - 22, x + 22, 128 + 22), fill=(255, 255, 255, a))
    save(im, 'animation')


def uidemo():
    im, d = tile((20, 150, 140, 255))
    d.rounded_rectangle((56, 70, 200, 102), radius=10, fill=WHITE)
    d.rounded_rectangle((56, 118, 140, 150), radius=10, fill=WHITE)
    d.ellipse((160, 116, 196, 152), fill=WHITE)
    d.rounded_rectangle((56, 166, 200, 186), radius=10, fill=(255, 255, 255, 170))
    save(im, 'uidemo')


os.makedirs(OUT, exist_ok=True)
terminal()
files()
settings()
animation()
uidemo()
window_icon('rustica-window', (200, 72, 40, 255))
window_icon('wintest', (110, 110, 120, 255))
# The web browser's entry: the browser app's globe
here = os.path.join(os.path.dirname(__file__), '..', 'userspace', 'apps', 'org.rustica.browser', 'icon.png')
shutil.copyfile(here, os.path.join(OUT, 'browser.png'))
print('icons in', OUT)
