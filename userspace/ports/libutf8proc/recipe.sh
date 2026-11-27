# libutf8proc (https://github.com/netsurf-browser/libutf8proc, 0d22740 2019-06-26; MIT (utf8proc), src/COPYING):
# the sources its own build compiles (taken from a host build's list), with
# the files that build generates (gperf through ports/tools/gperf-lite.py),
# and the flags NetSurf's build system gives GCC (_ALIGNED, STMTEXPR) and NDEBUG, as its release builds
F="-std=c99 -D_BSD_SOURCE -D_DEFAULT_SOURCE -Dinline=__inline__ -D_ALIGNED=__attribute__((aligned)) -DSTMTEXPR=1 -DNDEBUG -Isrc/include -Isrc/src -Isrc -Isrc/include/libutf8proc -DUTF8PROC_EXPORTS"
compile "$OBJ" "$F" src/src/utf8proc.c
archive utf8proc "$OBJ"
headers . src/include/libutf8proc
