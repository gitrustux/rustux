# libnsutils (https://github.com/netsurf-browser/libnsutils, 4f0b882 2026-11-15; MIT, src/COPYING):
# the sources its own build compiles (taken from a host build's list), with
# the files that build generates (gperf through ports/tools/gperf-lite.py),
# and the flags NetSurf's build system gives GCC (_ALIGNED, STMTEXPR) and NDEBUG, as its release builds
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -Dinline=__inline__ -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -DNDEBUG -Isrc/include -Isrc/src -Isrc "
compile "$OBJ" "$F" src/src/base64.c src/src/time.c src/src/unistd.c
archive nsutils "$OBJ"
headers . src/include/nsutils
