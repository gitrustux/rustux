# libnsfb (https://github.com/netsurf-browser/libnsfb, a287525 2026-11-15; MIT,
# src/COPYING): the library and its RAM surface unchanged; rustica.c is a
# surface of our own (a window of the compositor), and include/libnsfb.h
# gains its type, NSFB_SURFACE_RUSTICA. Programs link it with
# --whole-archive: the surfaces register themselves (constructors)
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -Isrc/include -Isrc/src"
P=src/src/plot
compile "$OBJ" "$F" src/src/libnsfb.c src/src/dump.c src/src/cursor.c src/src/palette.c \
    $P/api.c $P/util.c $P/generic.c $P/32bpp-xrgb8888.c $P/32bpp-xbgr8888.c $P/16bpp.c $P/8bpp.c \
    src/src/surface/surface.c src/src/surface/ram.c rustica.c
archive nsfb "$OBJ"
headers . src/include/libnsfb.h src/include/libnsfb_cursor.h src/include/libnsfb_event.h src/include/libnsfb_plot.h src/include/libnsfb_plot_util.h
