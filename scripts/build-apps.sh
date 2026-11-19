#!/bin/bash
# Build the apps of the apps repository (FoxDen step FD3, docs/FOXDEN.md)
# into app packages: one .rpg per folder of userspace/apps/ (RPG/control,
# manifest, icon), with its program from userspace/apps/bin/ (made by
# userspace/build-rust.sh from userspace/rust/foxden-apps).
#
#   scripts/build-apps.sh OUTDIR [ID...]
#
# Prints each file it makes. Uses rpg for this computer ($RPG, or built
# here with cargo). $APPS_SRC: another folder laid out as userspace/apps
# (the publishing test's).
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?usage: scripts/build-apps.sh OUTDIR [ID...]}; shift
mkdir -p "$OUT"
if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host")
    RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
fi
SRC=${APPS_SRC:-userspace/apps}
IDS=("$@")
[ ${#IDS[@]} -gt 0 ] || for d in "$SRC"/*/RPG/control; do IDS+=("$(basename "$(dirname "$(dirname "$d")")")"); done
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
for id in "${IDS[@]}"; do
    src=$SRC/$id
    [ -f "$src/RPG/control" ] || { echo "build-apps: $id: no $src/RPG/control"; exit 1; }
    prog=$(sed -n 's/^Exec: *//p' "$src/manifest" | awk '{ print $1 }')
    bin=$SRC/bin/$(basename "$prog")
    [ -x "$bin" ] || { echo "build-apps: $id: no program $bin (userspace/build-rust.sh makes it)"; exit 1; }
    d=$T/$id
    rm -rf "$d"
    cp -r "$src" "$d"
    mkdir -p "$d/$(dirname "$prog")"
    install -m 755 "$bin" "$d/$prog"
    v=$(sed -n 's/^Version: *//p' "$src/RPG/control")
    a=$(sed -n 's/^Architecture: *//p' "$src/RPG/control")
    f=$OUT/${id}_${v}_${a}.rpg
    "$RPG" build "$d" -o "$f" > /dev/null
    echo "$f"
done
