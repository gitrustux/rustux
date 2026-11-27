# zlib 1.3.1 (https://github.com/madler/zlib, tag v1.3.1; the zlib licence,
# src/LICENSE): unchanged
compile "$OBJ" "-DHAVE_UNISTD_H -DHAVE_STDARG_H" src/*.c
archive z "$OBJ"
headers . src/zlib.h src/zconf.h
