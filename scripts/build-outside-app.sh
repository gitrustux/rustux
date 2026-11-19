#!/bin/bash
# Build an outside developer's app from its source (FoxDen step FD7a,
# docs/FOXDEN.md "Outside developers"): the folder a developer submits to
# the foxden-apps repository, made into an app package.
#
#   scripts/build-outside-app.sh APPDIR OUTDIR
#
# APPDIR: manifest, RPG/control, the pictures, SIGNATURE, and src/ (Rust:
# src/main.rs and its modules). The app is a crate of its own, made here:
# its only dependency is the toolkit (rustica-ui, this tree's), built for
# Rustux as userspace/build-rust.sh builds the system's programs; the
# developer gives no Cargo.toml, so nothing of theirs runs while it is
# built (no build scripts, no other crates). Its program is put at the
# manifest's Exec, and rpg build makes OUTDIR/ID_VERSION_ARCH.rpg, which
# rpg dev check-package then checks against APPDIR. Prints the file.
#
# Needs what build-rust.sh needs (Rust with rust-src, gcc, ld), and
# builds the C library first if it is not built.
set -euo pipefail
R=$(cd "$(dirname "$0")/.." && pwd)
U=$R/userspace
APP=$(cd "${1:?usage: scripts/build-outside-app.sh APPDIR OUTDIR}" && pwd)
mkdir -p "${2:?usage: scripts/build-outside-app.sh APPDIR OUTDIR}"
OUT=$(cd "$2" && pwd)
die() { echo "build-outside-app: $*" >&2; exit 1; }
[ -f "$APP/manifest" ] && [ -f "$APP/RPG/control" ] && [ -f "$APP/src/main.rs" ] || die "$APP: needs manifest, RPG/control and src/main.rs"
field() { sed -n "s/^$1: *//p" "$2" | head -1 | tr -d '\r'; }
id=$(field Id "$APP/manifest")
ver=$(field Version "$APP/RPG/control")
arch=$(field Architecture "$APP/RPG/control")
prog=$(field Exec "$APP/manifest" | awk '{ print $1 }')
[ "$(field Package "$APP/RPG/control")" = "$id" ] || die "$APP: RPG/control's Package is not the manifest's Id $id"
[ "$arch" = x86_64 ] || die "$APP: Architecture x86_64 (an app is built for one)"
case $prog in ''|/*|*..*) die "$APP: Exec $prog: a path in the app's folder" ;; esac

if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd "$U/rust/rpg" && cargo build -q --release --target "$HOSTT" --target-dir "$U/build/rpg-host")
    RPG=$U/build/rpg-host/$HOSTT/release/rpg
fi
[ -f "$U/build/libc.a" ] && [ -f "$U/build/libc/crt0.o" ] || LIBC_ONLY=1 "$U/build.sh" > /dev/null

# The crate: made here, the developer's src/ in it
C=$U/build/outside/$id
rm -rf "$C"
mkdir -p "$C"
cp -r "$APP/src" "$C/src"
cat > "$C/Cargo.toml" <<TOML
[package]
name = "app"
version = "0.0.0"
edition = "2021"

[[bin]]
name = "app"
path = "src/main.rs"

[dependencies]
rustica-ui = { path = "$U/rust/rustica-ui" }

[profile.release]
panic = "abort"
opt-level = 3

[workspace]
TOML
# The toolkit's dependencies at the versions the system's apps use
cp "$U/rust/foxden-apps/Cargo.lock" "$C/Cargo.lock"

export RUSTC_BOOTSTRAP=1
export RUSTFLAGS="-C panic=abort -C relocation-model=static -C target-feature=+crt-static -C linker=$U/rust/rxld.sh -C linker-flavor=ld -C link-self-contained=no --cfg rustix_use_libc"
TARGET=x86_64-unknown-linux-musl
(cd "$C" && cargo build -q --release --target $TARGET -Zbuild-std=std,panic_abort --target-dir "$U/build/rust-outside") ||
    die "$id: the build failed (above)"

# The package: the folder without src/ and SIGNATURE, with the program
P=$U/build/outside/$id.pkg
rm -rf "$P"
mkdir -p "$P"
(cd "$APP" && tar cf - --exclude=./src --exclude=./SIGNATURE .) | tar xf - -C "$P"
mkdir -p "$P/$(dirname "$prog")"
strip -o "$P/$prog" "$U/build/rust-outside/$TARGET/release/app"
chmod 755 "$P/$prog"
f=$OUT/${id}_${ver}_${arch}.rpg
"$RPG" build "$P" -o "$f" > /dev/null
"$RPG" dev check-package "$f" "$APP" > /dev/null
echo "$f"
