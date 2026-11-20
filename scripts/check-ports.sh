#!/bin/bash
# Check that the code meant to be portable still builds for the
# architectures Rustux is being ported to: aarch64 and riscv64
# (docs/PORTING.md). Nothing is run and nothing is installed; this only
# compiles, so it catches x86 code that crept into portable code.
#
#   scripts/check-ports.sh            the C library, the C programs and
#                                     every Rust program
#   scripts/check-ports.sh --quick    C only (seconds instead of minutes)
#
# Needs clang (it cross-compiles without a cross gcc) and, for the Rust
# part, the same toolchain as userspace/build-rust.sh (rust-src; the
# targets' std is rebuilt, so they need not be installed).
#
# The kernel's own rule (no asm! outside src/arch, except what
# src/arch/NOT-YET-MOVED still lists) is checked by build.rs on every
# kernel build.
set -uo pipefail
cd "$(dirname "$0")/../userspace"
U=$PWD
QUICK=0
[ "${1:-}" = --quick ] && QUICK=1
command -v clang >/dev/null || { echo "check-ports: needs clang" >&2; exit 2; }
RES=$(clang -print-resource-dir)/include
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
fails=0
fail() { echo "  FAIL $*"; fails=$((fails + 1)); }

# Programs that are x86-only by nature (one per line, '#' comments); a
# port brings its own or leaves them out
mapfile -t X86_ONLY < <(sed 's/#.*//' X86-ONLY | awk 'NF {print $1}')
x86_only() { local n; for n in "${X86_ONLY[@]}"; do [ "$n" = "$1" ] && return 0; done; return 1; }

echo "check-ports: assembly in the Rust programs"
# Rust programs reach the kernel through the C library's __rx_syscall
hits=$(grep -rln 'asm!' --include=*.rs rust | grep -v '^rust/vendor/' || true)
[ -z "$hits" ] || fail "asm! in Rust programs (use __rx_syscall from the C library): $hits"

for arch in aarch64 riscv64; do
    t=$arch-linux-gnu
    echo "check-ports: C for $arch"
    CF="--target=$t -std=gnu11 -O2 -Wall -Wextra -Werror=implicit-function-declaration -ffreestanding
        -fno-stack-protector -nostdinc -isystem libc/include -isystem $RES"
    for src in libc/src/*.c libc/src/arch/$arch.c; do
        out=$(clang $CF -c "$src" -o "$TMP/o.o" 2>&1) || fail "$src: $(echo "$out" | grep -m1 error)"
    done
    for src in c-progs/*.c; do
        name=$(basename "${src%.c}")
        if clang $CF -Ic-progs -c "$src" -o "$TMP/o.o" >"$TMP/log" 2>&1; then
            x86_only "$name" && fail "$name builds for $arch now: take it off userspace/X86-ONLY"
        else
            x86_only "$name" || fail "$src: $(grep -m1 error "$TMP/log")"
        fi
    done
done

if [ $QUICK = 0 ]; then
    export RUSTC_BOOTSTRAP=1 RUSTFLAGS="--cfg rustix_use_libc"
    for T in aarch64-unknown-linux-musl riscv64gc-unknown-linux-musl; do
        echo "check-ports: Rust for $T"
        tt=${T//-/_}; ct=${T/gc-/-}; ct=${ct/-unknown/}
        export CC_$tt=clang AR_$tt=llvm-ar
        export CFLAGS_$tt="--target=$ct -ffreestanding -nostdinc -isystem $RES -I$U/libc/include -fno-stack-protector -fno-builtin -O2"
        for dir in rust/*/; do
            [ -f "$dir/Cargo.toml" ] || continue
            name=$(basename "$dir")
            CARGO_ARGS=()
            [ -f "$dir/rustux-build" ] && . "$dir/rustux-build"
            (cd "$dir" && cargo check --release --target $T -Zbuild-std=std,panic_abort \
                --target-dir "$U/build/check-ports" "${CARGO_ARGS[@]}") >"$TMP/log" 2>&1 \
                || fail "rust/$name for $T: $(grep -m1 '^error' "$TMP/log")"
        done
    done
fi

if [ $fails = 0 ]; then
    echo "check-ports: all portable code builds for aarch64 and riscv64"
else
    echo "check-ports: $fails FAILED"
    exit 1
fi
