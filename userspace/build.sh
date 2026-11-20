#!/bin/bash
# Build the Rustux C library and the C programs into userspace/bin/.
# The kernel build (build.rs) embeds every file in userspace/bin/ in the
# ramdisk as /bin/<name>. The binaries are committed, so building the
# kernel does not need this script; run it after changing C code.
#
# Needs: gcc for x86-64 (any Linux distribution's gcc works), ar.
set -euo pipefail
cd "$(dirname "$0")"

CC=${CC:-gcc}
AR=${AR:-ar}
OUT=build
GCCINC=$($CC -print-file-name=include)
LIBGCC=$($CC -print-libgcc-file-name)
CFLAGS="-std=gnu11 -O2 -Wall -Wextra -ffreestanding -fno-stack-protector -fno-pie -fno-pic
        -fno-asynchronous-unwind-tables -nostdinc -isystem $GCCINC -isystem libc/include"

mkdir -p "$OUT/libc" bin

# The library
objs=()
# (libc/src/arch/: x86_64.c, this architecture's part; docs/PORTING.md)
for src in libc/src/*.c libc/src/arch/x86_64.c; do
    obj="$OUT/libc/$(basename "${src%.c}").o"
    [[ $src == */arch/* ]] && obj="$OUT/libc/arch-$(basename "${src%.c}").o"
    $CC $CFLAGS -c "$src" -o "$obj"
    [[ $src == */crt0.c ]] || objs+=("$obj")
done
rm -f "$OUT/libc.a"
$AR rcs "$OUT/libc.a" "${objs[@]}"
# LIBC_ONLY=1: the library alone (scripts/build-outside-app.sh, for an
# outside developer's app: FoxDen step FD7a)
[ "${LIBC_ONLY:-}" = 1 ] && exit 0

# The programs: one per .c file in c-progs/
for src in c-progs/*.c; do
    name=$(basename "${src%.c}")
    $CC $CFLAGS -Ic-progs -c "$src" -o "$OUT/$name.o"
    $CC -static -nostdlib -no-pie -Wl,--build-id=none -Wl,-z,noseparate-code -T libc/rustux.ld \
        "$OUT/libc/crt0.o" "$OUT/$name.o" "$OUT/libc.a" "$LIBGCC" -o "bin/$name"
done
echo "built: $(ls bin | tr '\n' ' ')"
