# NetSurf (https://github.com/netsurf-browser/netsurf, 170d894 2026-11-18; GPL 2,
# src/COPYING): the browser's core and its framebuffer frontend, without
# the other frontends, its tests and documents. Built by its own Makefile
# (TARGET=framebuffer; src/Makefile.config: no JavaScript yet, FreeType
# fonts from /usr/share/fonts/noto) with tools/rx-cc as the compiler and
# the ports' pkg-config files, in a copy of src/ under build/. Our changes
# are applied in src/ and listed in rustica.patch: download.c (downloads
# to ~/Downloads), the layout's text for key presses, the Rustica surface
# chosen first, a window that fits the desktop, no regex or mmap on
# Rustux, the root's and body's overflow scrolled by the window, the
# compositor's pointer shapes. NetSurf is an app (org.rustica.browser,
# docs/BROWSER.md): output userspace/apps/bin/netsurf and its resources in
# userspace/apps/org.rustica.browser/share/netsurf.
B="$OBJ/tree"
# A fresh tree each time: NetSurf's Makefile does not rebuild what
# Makefile.config's paths are compiled into when only they change
rm -rf "$B"
mkdir -p "$B"
cp -a src/. "$B/"
../tools/make-pc.sh >/dev/null
PKG_CONFIG_LIBDIR="$SYSROOT/lib/pkgconfig" PKG_CONFIG_PATH= make -s -C "$B" -j"$JOBS" \
    CC="$(cd ../tools && pwd)/rx-cc" BUILD_CC=gcc HOST=rustux TARGET=framebuffer
A="$U/apps"
cp "$B/nsfb" "$A/bin/netsurf"
# without its debugging information (its Makefile builds with -g): half the size
strip "$A/bin/netsurf"
R="$A/org.rustica.browser/share/netsurf"
mkdir -p "$R"
for f in adblock.css default.css internal.css quirks.css favicon.png netsurf.png; do cp -L "$B/frontends/framebuffer/res/$f" "$R/"; done
cp -L "$B/frontends/framebuffer/res/en/Messages" "$B/frontends/framebuffer/res/en/welcome.html" \
    "$B/frontends/framebuffer/res/en/credits.html" "$B/frontends/framebuffer/res/en/licence.html" "$R/"
