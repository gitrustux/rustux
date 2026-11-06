#!/bin/bash
# Build the Rust programs in userspace/rust/ (one Cargo package each) into
# userspace/bin/. Like the C programs, the binaries are committed; run
# this after changing them. docs/RUST.md explains how Rust's std runs on
# Rustux.
#
# Needs: a Rust toolchain with the rust-src component (std is rebuilt for
# x86_64-unknown-linux-musl, so the musl target itself need not be
# installed), gcc and ld, and the C library (built by build.sh).
set -euo pipefail
cd "$(dirname "$0")"
U=$PWD
[ -f build/libc.a ] || ./build.sh

# -Zbuild-std is unstable; RUSTC_BOOTSTRAP lets a stable toolchain use it
export RUSTC_BOOTSTRAP=1
export RUSTFLAGS="-C panic=abort -C relocation-model=static -C target-feature=+crt-static
                  -C linker=$U/rust/rxld.sh -C linker-flavor=ld -C link-self-contained=no
                  --cfg rustix_use_libc"
TARGET=x86_64-unknown-linux-musl
# C code inside crates (oniguruma, under find) is compiled against the
# Rustux C library's headers, not the build machine's
export CC_x86_64_unknown_linux_musl=${CC:-gcc} AR_x86_64_unknown_linux_musl=${AR:-ar}
export CFLAGS_x86_64_unknown_linux_musl="-ffreestanding -nostdinc -isystem $(${CC:-gcc} -print-file-name=include)
    -I$U/libc/include -fno-stack-protector -fno-pie -fno-builtin -O2"
for dir in rust/*/; do
    [ -f "$dir/Cargo.toml" ] || continue
    name=$(basename "$dir")
    CARGO_ARGS=()
    [ -f "$dir/rustux-build" ] && . "$dir/rustux-build"
    # cargo doesn't see changes to the C library: always link again
    rm -f "build/rust/$TARGET/release/$name" "build/rust/$TARGET/release/deps/${name//-/_}"-*
    (cd "$dir" && cargo build --release --target $TARGET -Zbuild-std=std,panic_abort \
        --target-dir "$U/build/rust" "${CARGO_ARGS[@]}")
    strip -o "bin/$name" "build/rust/$TARGET/release/$name"
done
echo "built: $(cd rust && ls -d */Cargo.toml | cut -d/ -f1 | tr '\n' ' ')"
