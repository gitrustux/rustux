# libparserutils (https://github.com/netsurf-browser/libparserutils, c997126 2026-11-15; MIT, src/COPYING):
# the sources its own build compiles (taken from a host build's list), with
# the files that build generates (gperf through ports/tools/gperf-lite.py),
# and the flags NetSurf's build system gives GCC (_ALIGNED, STMTEXPR) and NDEBUG, as its release builds
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -Dinline=__inline__ -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -DNDEBUG -Isrc/include -Isrc/src -Isrc -DWITHOUT_ICONV_FILTER"
compile "$OBJ" "$F" src/src/charset/aliases.c src/src/charset/codec.c src/src/charset/codecs/codec_ascii.c src/src/charset/codecs/codec_8859.c src/src/charset/codecs/codec_ext8.c src/src/charset/codecs/codec_utf8.c src/src/charset/codecs/codec_utf16.c src/src/charset/encodings/utf8.c src/src/charset/encodings/utf16.c src/src/input/filter.c src/src/input/inputstream.c src/src/utils/buffer.c src/src/utils/errors.c src/src/utils/stack.c src/src/utils/vector.c
archive parserutils "$OBJ"
headers . src/include/parserutils
