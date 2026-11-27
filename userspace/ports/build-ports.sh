#!/bin/bash
# Build the ported C libraries (userspace/ports/NAME/) into a sysroot:
# build/sysroot/include and build/sysroot/lib/libNAME.a, for programs that
# need more than the C library (the NetSurf browser, phase W).
#
# Each port is NAME/src/ (the upstream sources we use, unchanged unless its
# README says otherwise) and NAME/recipe.sh, sourced here: it calls
# compile (sources to objects with the port's flags), archive and headers.
# Ports are built in the order of PORTS; one name as the argument builds
# that one alone (its dependencies must be built already).
#
# Needs what userspace/build.sh needs (gcc for x86-64, ar), and the C
# library built by it (build/libc.a).
set -euo pipefail
cd "$(dirname "$0")"
U=$(cd .. && pwd)
CC=${CC:-gcc}
AR=${AR:-ar}
GCCINC=$($CC -print-file-name=include)
SYSROOT=$U/build/sysroot
JOBS=${JOBS:-$(nproc)}
mkdir -p "$SYSROOT/include" "$SYSROOT/lib"

# What every port is compiled with: the Rustux C library's headers, no
# hosted headers of the build machine, static code
BASE_CFLAGS="-O2 -g0 -ffreestanding -fbuiltin -fno-stack-protector -fno-pie -fno-pic -fno-asynchronous-unwind-tables"
BASE_CFLAGS+=" -nostdinc -isystem $GCCINC -isystem $U/libc/include -isystem $SYSROOT/include -D__rustux__ -w"

PORTS=(zlib mbedtls curl libpng libjpeg-turbo freetype
       libwapcaplet libparserutils libhubbub libcss libdom libnsutils libnsbmp libnsgif libnspsl libutf8proc libnsfb netsurf)

# compile OUTDIR FLAGS SOURCES...: one object per source (the source's path
# with / as _), JOBS at a time; only sources newer than their object
compile() {
    local out=$1 flags=$2 src o bad=0; shift 2
    mkdir -p "$out"
    for src in "$@"; do
        o="$out/$(echo "$src" | tr / _ | sed 's/\.c$/.o/')"
        [ -f "$o" ] && [ ! "$src" -nt "$o" ] && continue
        # shellcheck disable=SC2086
        { $CC $BASE_CFLAGS $flags -c "$src" -o "$o" || { echo "port: failed: $src" >&2; exit 1; }; } &
        while [ "$(jobs -rp | wc -l)" -ge "$JOBS" ]; do wait -n || bad=1; done
    done
    while [ "$(jobs -rp | wc -l)" -gt 0 ]; do wait -n || bad=1; done
    wait || bad=1
    [ $bad = 0 ] || { echo "port: compiling failed" >&2; return 1; }
}

# archive LIBNAME OBJDIR: libLIBNAME.a in the sysroot
archive() {
    rm -f "$SYSROOT/lib/lib$1.a"
    $AR rcs "$SYSROOT/lib/lib$1.a" "$2"/*.o
}

# headers DEST-SUBDIR FILES...: copied to the sysroot's include/DEST
headers() {
    local d="$SYSROOT/include/$1"; shift
    mkdir -p "$d"
    cp -r "$@" "$d/"
}

build_port() {
    local name=$1
    echo "== port $name"
    (
        cd "$name"
        OBJ=$U/build/ports/$name
        # shellcheck source=/dev/null
        . ./recipe.sh
    )
}

if [ $# -gt 0 ]; then
    for p in "$@"; do build_port "$p"; done
else
    for p in "${PORTS[@]}"; do [ -d "$p" ] && build_port "$p"; done
fi
echo "ports: $(ls "$SYSROOT/lib" | tr '\n' ' ')"

# The programs that use them: progs/NAME.c, its libraries named on a
# "// link: A B ..." line (-lA -lB ...), linked as userspace/build.sh links
# the C programs, into userspace/bin/
[ $# -gt 0 ] && exit 0
mkdir -p "$U/build/ports/progs"
LIBGCC=$($CC -print-libgcc-file-name)
for src in progs/*.c; do
    [ -f "$src" ] || continue
    name=$(basename "${src%.c}")
    libs=$(sed -n 's|^// link: ||p' "$src" | head -1)
    # shellcheck disable=SC2086
    $CC $BASE_CFLAGS -Wall -c "$src" -o "$U/build/ports/progs/$name.o"
    # shellcheck disable=SC2046,SC2086
    $CC -static -nostdlib -no-pie -Wl,--build-id=none -Wl,-z,noseparate-code -T "$U/libc/rustux.ld" \
        "$U/build/libc/crt0.o" "$U/build/ports/progs/$name.o" -L"$SYSROOT/lib" $(for l in $libs; do echo "-l$l"; done) \
        "$U/build/libc.a" "$LIBGCC" -o "$U/bin/$name"
    echo "built: $name"
done
