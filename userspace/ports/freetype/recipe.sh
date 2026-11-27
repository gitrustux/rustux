# FreeType 2.13.3 (https://github.com/freetype/freetype, tag VER-2-13-3; the
# FreeType licence, src/FTL.TXT): the sources of the modules we use,
# unchanged; ftmodule.h is our module list (TrueType, CFF, hinting, the
# anti-aliasing renderer); gzip with its own zlib copy, as by default
F="-Isrc/include -I. -DFT2_BUILD_LIBRARY -DFT_CONFIG_MODULES_H=<ftmodule.h>"
B=src/src/base
compile "$OBJ" "$F" $B/ftsystem.c $B/ftinit.c $B/ftdebug.c $B/ftbase.c $B/ftbbox.c $B/ftglyph.c $B/ftbitmap.c \
    $B/ftsynth.c $B/ftstroke.c $B/ftmm.c $B/fttype1.c \
    src/src/truetype/truetype.c src/src/sfnt/sfnt.c src/src/smooth/smooth.c src/src/psnames/psnames.c \
    src/src/autofit/autofit.c src/src/cff/cff.c src/src/psaux/psaux.c src/src/pshinter/pshinter.c \
    src/src/gzip/ftgzip.c src/src/raster/raster.c src/src/cache/ftcache.c
archive freetype "$OBJ"
headers freetype2 src/include/ft2build.h src/include/freetype
