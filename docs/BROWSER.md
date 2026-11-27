# The web browser (phase W)

Rustica's web browser is **NetSurf** (https://www.netsurf-browser.org,
GPL 2), ported 2026-11-26: a small browser written in C with its own HTML,
CSS and layout engine (HTML 4 and much of HTML 5's markup, CSS 2.1 and
parts of CSS 3). It is `/bin/netsurf`, "Web Browser" in the menu
(Internet), and Files opens `.html` files with it.

```
netsurf [URL or /path/to/file.html]
netsurf -v URL          # its log on stderr
```

## How it is built

NetSurf runs on its **framebuffer frontend** (the one NetSurf keeps for
Linux framebuffers and SDL), over **libnsfb**, which gained a surface of
our own, `rustica` (`userspace/ports/libnsfb/rustica.c`): a window of the
compositor (`<rustux/window.h>`), its buffer the framebuffer, a commit for
each changed box, the compositor's pointer. Keys come as HID usages; a key
press that makes text comes with the text the user's keyboard layout
makes, so every layout types what it should.

Everything it needs is ported in `userspace/ports/` (`README.md` there):
libcurl over mbedTLS for HTTP and HTTPS, FreeType for text, libpng,
libjpeg-turbo, NetSurf's own libraries (HTML parser, DOM, CSS, images).
`ports/build-ports.sh` builds them and NetSurf (with NetSurf's own Makefile
and `tools/rx-cc`); the program is committed in `userspace/bin/` like the
other C programs.

Our changes to NetSurf are in `userspace/ports/netsurf/rustica.patch`
(applied in `src/`):

- `frontends/framebuffer/download.c` (new): downloads, which the
  framebuffer frontend did not have (below);
- the layout's text for key presses (`fbtk/event.c`);
- the surface with the lowest type chosen by default, as intended (the
  frontend never kept the lowest it had seen, so the last one registered
  won); the Rustica surface comes first in libnsfb;
- a window of 1100x680 instead of the whole screen; a file's path as the
  argument becomes its `file:` URL;
- no POSIX regex (only "save complete page" used it) and no `mmap` of
  files (Rustux's `mmap` does not map files): NetSurf's own fallbacks;
- the build's configuration (`Makefile.config`): no JavaScript yet,
  FreeType with the Noto fonts.

## Fonts

Noto Sans, Noto Serif and Noto Sans Mono, with bold and italic, in
`/usr/share/fonts/noto` (OFL, `OFL.txt` there), for sans-serif, serif and
monospace (cursive and fantasy use Noto Serif Italic and Noto Sans Bold).

## Downloads

A link to something NetSurf does not show (or a download a site asks for)
is saved in `~/Downloads`: written as `NAME.part`, renamed `NAME` when it
is complete (`NAME (2).EXT` and so on if the name is taken). The window's
status line shows the progress ("Downloading NAME: 45% (1.2 MB of 2.7 MB)");
a notification says when it is done or failed.

## HTTPS

libcurl with mbedTLS (TLS 1.2 and 1.3); the certificates it trusts are
Mozilla's (`/etc/ssl/certs/ca-certificates.crt`, from certifi 2026.10.18,
MPL 2.0). Entropy comes from the C library's `getrandom` (the CPU's RDRAND
mixed with the clocks).

## Not yet

- **JavaScript**: NetSurf's JavaScript (Duktape) needs its bindings
  generator (nsgenbind, which needs flex); pages work without it as NetSurf
  renders them, but scripts do not run.
- **Bookmarks and history windows, tabs**: NetSurf's core has them; the
  framebuffer frontend has no windows for them. The next step is a
  Rustica frontend in rustica-ui (toolbar, tabs, bookmarks, a downloads
  list, the theme's colours) on the same core.
- SVG and WebP images, video.
- A modern engine (Servo or Ladybird) remains the goal for later
  (`CHECKLIST.md` phase W).

## Test

`scripts/run-browser-test.sh`: QEMU with the desktop and web servers on
the host (HTTP and HTTPS with a test CA): a page rendered (its colours),
a link clicked downloads a file with the same bytes, a URL typed in the
address bar loads over HTTPS, a local file by its path, an unknown CA
refused. The network test also checks libcurl (`curltest`).
