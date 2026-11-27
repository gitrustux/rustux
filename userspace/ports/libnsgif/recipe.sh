# libnsgif (https://github.com/netsurf-browser/libnsgif, b9e3b0a 2026-11-18; MIT, src/COPYING):
# the sources its own build compiles (taken from a host build's list), with
# the files that build generates (gperf through ports/tools/gperf-lite.py),
# and the flags NetSurf's build system gives GCC (_ALIGNED, STMTEXPR) and NDEBUG, as its release builds
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -Dinline=__inline__ -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -DNDEBUG -Isrc/include -Isrc/src -Isrc "
compile "$OBJ" "$F" src/src/gif.c src/src/lzw.c
archive nsgif "$OBJ"
headers . src/include/nsgif.h
