#!/bin/bash
# pkg-config files for the ports (sysroot/lib/pkgconfig), so build systems
# that ask pkg-config (NetSurf's) find them: NAME VERSION LIBS [REQUIRES]
set -e
U=$(cd "$(dirname "$0")/../.." && pwd)
D=$U/build/sysroot/lib/pkgconfig
mkdir -p "$D"
pc() {
    cat > "$D/$1.pc" <<PC
prefix=$U/build/sysroot
libdir=\${prefix}/lib
includedir=\${prefix}/include
Name: $1
Description: $1 (a Rustux port)
Version: $2
Requires: $4
Libs: -L\${libdir} $3
Cflags: -I\${includedir} $5
PC
}
pc libwapcaplet 0.4.3 -lwapcaplet
pc libparserutils 0.2.5 -lparserutils
pc libhubbub 0.3.8 -lhubbub libparserutils
pc libcss 0.9.2 -lcss "libparserutils libwapcaplet"
pc libdom 0.4.2 -ldom "libhubbub libparserutils libwapcaplet"
pc libnsutils 0.1.1 -lnsutils
pc libutf8proc 2.4.0 -lutf8proc "" -I\${includedir}/libutf8proc
pc libnspsl 0.1.7 -lnspsl
pc libnsbmp 0.1.7 -lnsbmp
pc libnsgif 1.0.0 -lnsgif
pc libnsfb 0.2.2 -lnsfb
pc zlib 1.3.1 -lz
pc libpng 1.6.44 "-lpng" zlib
pc libpng16 1.6.44 "-lpng" zlib
pc libjpeg 3.0.4 -ljpeg
pc freetype2 26.2.20 -lfreetype "" -I\${includedir}/freetype2
pc libcurl 8.11.1 "-lcurl -lmbedtls -lmbedx509 -lmbedcrypto -lz" "" -DCURL_STATICLIB
echo "pkg-config files: $(ls "$D" | tr '\n' ' ')"
