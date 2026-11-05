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
                  -C linker=$U/rust/rxld.sh -C linker-flavor=ld -C link-self-contained=no"
TARGET=x86_64-unknown-linux-musl
for dir in rust/*/; do
    [ -f "$dir/Cargo.toml" ] || continue
    name=$(basename "$dir")
    # cargo doesn't see changes to the C library: always link again
    rm -f "build/rust/$TARGET/release/$name" "build/rust/$TARGET/release/deps/${name//-/_}"-*
    (cd "$dir" && cargo build --release --target $TARGET -Zbuild-std=std,panic_abort \
        --target-dir "$U/build/rust")
    strip -o "bin/$name" "build/rust/$TARGET/release/$name"
done
echo "built: $(cd rust && ls -d */ | tr -d / | tr '\n' ' ')"
