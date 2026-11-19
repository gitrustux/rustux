#!/bin/bash
# The outside developers' apps for the apps repository (FoxDen step FD7a,
# docs/FOXDEN.md "Outside developers"): from a checkout of the
# foxden-apps repository's main, each app's current version as its
# publish workflow released it on GitHub, checked here before
# scripts/publish-apps.sh --add signs it in with the store's key.
#
#   scripts/fetch-outside-apps.sh FOXDEN_APPS OUTDIR [--from DIR]
#
# For each apps/ID (ID_VERSION_x86_64.rpg, from the release ID-VERSION;
# --from DIR: from DIR instead, for the tests):
#   - its developer is in developers/ and the app's ID is under theirs,
#     and not the project's own (org.rustica, ...);
#   - the folder is signed by the developer's key (rpg dev verify);
#   - the package is the folder built: the same control fields and files,
#     and only the program added (rpg dev check-package);
#   - when gh is signed in: the package's build provenance (gh attestation
#     verify: built by the repository's workflow).
# An app whose release is not there yet (the workflow still running) is
# skipped with a word; any other problem stops it (exit 1), nothing in
# OUTDIR. Prints a line per app.
set -euo pipefail
cd "$(dirname "$0")/.."
SRC=${1:?usage: scripts/fetch-outside-apps.sh FOXDEN_APPS OUTDIR [--from DIR]}
OUT=${2:?usage: scripts/fetch-outside-apps.sh FOXDEN_APPS OUTDIR [--from DIR]}
FROM=""
[ "${3:-}" = --from ] && FROM=${4:?--from DIR}
REPO_URL=${FOXDEN_APPS_URL:-https://github.com/gitrustux/foxden-apps}
die() { echo "fetch-outside-apps: $*"; exit 1; }
if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host")
    RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
fi
attest=no
if command -v gh > /dev/null && gh auth status > /dev/null 2>&1; then attest=yes; fi
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$OUT"
n=0
for c in "$SRC"/apps/*/RPG/control; do
    [ -f "$c" ] || continue
    d=$(dirname "$(dirname "$c")")
    id=$(basename "$d")
    v=$(sed -n 's/^Version: *//p' "$c" | tr -d '\r')
    dev=$(sed -n 's/^Developer: *//p' "$d/manifest" | tr -d '\r')
    case $id in org.rustica.*|org.rustux.*|com.rustux.*) die "$id: the project's own namespace" ;; esac
    [ -n "$dev" ] && [ -f "$SRC/developers/$dev/key.pub" ] || die "$id: its developer ${dev:-(none)} is not in developers/"
    case $id in "$dev".*) ;; *) die "$id: not under its developer's ID $dev" ;; esac
    "$RPG" dev verify "$d" "$SRC/developers/$dev/key.pub" > /dev/null || die "$id: not signed by its developer's key"
    f=${id}_${v}_x86_64.rpg
    if [ -n "$FROM" ]; then
        [ -f "$FROM/$f" ] || { echo "fetch-outside-apps: $id $v: not built yet (skipped)"; continue; }
        cp "$FROM/$f" "$T/$f"
    elif ! curl -fsSL -o "$T/$f" "$REPO_URL/releases/download/$id-$v/$f"; then
        echo "fetch-outside-apps: $id $v: no release yet (the publish workflow still running?): skipped"
        continue
    fi
    "$RPG" dev check-package "$T/$f" "$d" > /dev/null || die "$id $v: the released package is not its folder built"
    how="provenance not checked (gh is not signed in)"
    if [ $attest = yes ]; then
        gh attestation verify "$T/$f" --repo "${REPO_URL#https://github.com/}" > /dev/null 2>&1 ||
            die "$id $v: its build provenance does not check (gh attestation verify)"
        how="provenance checked"
    fi
    mv "$T/$f" "$OUT/$f"
    echo "fetch-outside-apps: $id $v by $dev: signed, the folder built, $how"
    n=$((n + 1))
done
echo "fetch-outside-apps: $n apps in $OUT"
