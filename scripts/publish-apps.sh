#!/bin/bash
# Publish the apps as the signed apps repository (FoxDen step FD3,
# docs/FOXDEN.md): what https://rustux.com/download/apps serves.
#
#   scripts/publish-apps.sh --key KEYFILE [--pub PUBFILE] [--keep N] APPSDIR
#
# 1. The key: an Ed25519 private key (OpenSSL's PEM) whose public half
#    must be the apps key the image trusts (files/etc/rpg/keys/apps/
#    rustux-apps-2026.pub, or --pub, for tests): never the archive key.
#    It must not be inside the web root (the folder two above APPSDIR) or
#    under /var/www.
# 2. The apps (scripts/build-apps.sh: every folder of userspace/apps/) are
#    made in a staging folder. An app whose version the signed index lists
#    already must be the same file (published packages never change);
#    when every app is published already there is nothing to do.
# 3. A new index of each app's newest N versions (default 2) is made in
#    the staging folder (rpg index --apps: platform/1/, media/), Release
#    signed with the key, then checked as a client would: rpg update, with
#    only the public key, from that folder.
# 4. Only then the new packages go into the pool, the new platform/ and
#    media/ replace the old ones, and older versions' files go. A failed
#    run leaves the repository as it was.
#
# Needs openssl 3, cargo (rpg for this computer, unless $RPG) and the apps'
# programs in userspace/apps/bin (userspace/build-rust.sh).
set -euo pipefail
cd "$(dirname "$0")/.."
KEY="" PUB=files/etc/rpg/keys/apps/rustux-apps-2026.pub KEEP=2 REPO="" LEVEL=1
while [ $# -gt 0 ]; do
    case $1 in
        --key) KEY=$2; shift 2 ;;
        --pub) PUB=$2; shift 2 ;;
        --keep) KEEP=$2; shift 2 ;;
        -*) echo "publish-apps: $1?"; exit 2 ;;
        *) REPO=$1; shift ;;
    esac
done
[ -n "$KEY" ] && [ -n "$REPO" ] || { echo "usage: scripts/publish-apps.sh --key KEYFILE [--pub PUBFILE] [--keep N] APPSDIR"; exit 2; }
[ "$KEEP" -ge 1 ] 2>/dev/null || { echo "publish-apps: --keep needs 1 or more"; exit 2; }
die() { echo "publish-apps: $*"; exit 1; }
command -v openssl >/dev/null || die "openssl is needed"

# --- 1. the key
[ -r "$KEY" ] || die "$KEY: cannot read the key"
mkdir -p "$REPO"
REPO=$(cd "$REPO" && pwd)
KEYREAL=$(realpath "$KEY")
WEBROOT=$(dirname "$(dirname "$REPO")")
case $KEYREAL in
    /var/www/*|"$WEBROOT"/*) die "$KEYREAL is inside the web root: keep the apps key out of it (and out of git)" ;;
esac
if [ -n "$(find "$KEYREAL" -perm /077 2>/dev/null)" ]; then
    echo "publish-apps: warning: $KEYREAL can be read by others than its owner (chmod 600 it)"
fi
want=$(awk '$1 == "rpg-ed25519" { print $2 }' "$PUB")
have=$(openssl pkey -in "$KEY" -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')
[ -n "$have" ] || die "$KEY: not an Ed25519 private key openssl can read"
[ "$have" = "$want" ] || die "$KEY is not the apps key the image trusts ($PUB): refused, nothing changed"

# --- rpg for this computer
if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host")
    RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
fi
export RPG

# --- 2. the apps, into the staging folder only
T=$(mktemp -d)
STAGE="$REPO/.staging"
trap 'rm -rf "$T" "$STAGE"' EXIT
indexed() { find "$REPO/platform/$LEVEL" -name Apps.gz -exec zcat {} + 2>/dev/null | sed -n 's/^Filename: //p'; }
rm -rf "$STAGE"
mkdir -p "$STAGE/pool"
scripts/build-apps.sh "$T/new" > /dev/null
NEW=()
for f in "$T"/new/*.rpg; do
    rel=$("$RPG" pool-path "$f")
    if indexed | grep -qxF "$rel"; then
        cmp -s "$f" "$REPO/$rel" || die "$rel is published with other contents: a version is never published twice (raise its Version)"
        continue
    fi
    mkdir -p "$STAGE/$(dirname "$rel")"
    install -m 644 "$f" "$STAGE/$rel"
    NEW+=("$rel")
done
if [ ${#NEW[@]} -eq 0 ] && [ -f "$REPO/platform/$LEVEL/Release.sig" ]; then
    echo "publish-apps: every app is published already (published packages never change): nothing to do"
    exit 0
fi

# --- 3. each app's newest KEEP versions, indexed and signed in the
# staging folder (ID_VERSION_ARCH.rpg: IDs have no "_")
keep() { # the pool files to keep, relative to the pool (L/ID/FILE)
    find "$REPO/pool" "$STAGE/pool" -name '*.rpg' -printf '%P\n' 2>/dev/null | sort -u > "$T/all"
    for id in $(awk -F/ '{ print $(NF-1) }' "$T/all" | sort -u); do
        grep "/$id/" "$T/all" | while IFS= read -r rel; do echo "$(basename "$rel" | cut -d_ -f2) $rel"; done |
            sort -V -k1,1 | tail -n "$KEEP" | cut -d' ' -f2
    done
}
KEPT=$(keep)
while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    [ -e "$STAGE/pool/$rel" ] && continue
    mkdir -p "$STAGE/pool/$(dirname "$rel")"
    ln "$REPO/pool/$rel" "$STAGE/pool/$rel" 2>/dev/null || cp "$REPO/pool/$rel" "$STAGE/pool/$rel"
done <<< "$KEPT"
n=$(find "$STAGE/pool" -name '*.rpg' | wc -l)
"$RPG" index --apps "$STAGE" --platform "$LEVEL" > /dev/null
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$STAGE/platform/$LEVEL/Release" -out "$STAGE/platform/$LEVEL/Release.sig"
chmod -R a+rX "$STAGE/platform" "$STAGE/pool" "$STAGE/media"
# As a client: only the public key, rpg update from the staging folder
C=$T/client
mkdir -p "$C/etc/rpg/keys/apps"
cp "$PUB" "$C/etc/rpg/keys/apps/apps.pub"
echo "apps file://$STAGE  platform $LEVEL" > "$C/etc/rpg/sources.list"
out=$("$RPG" --root "$C" update 2>&1) || die "the new index does not pass rpg update (nothing published): $out"
grep -q "file://$STAGE platform $LEVEL: $n apps" <<< "$out" || die "rpg update read something else than the $n apps (nothing published): $out"

# --- 4. the new packages into the pool, the new index and media in place,
# then older versions go
for rel in "${NEW[@]}"; do
    mkdir -p "$REPO/$(dirname "$rel")"
    install -m 644 "$STAGE/$rel" "$REPO/$rel"
done
for d in platform media; do
    rm -rf "$REPO/$d.old"
    [ -d "$REPO/$d" ] && mv "$REPO/$d" "$REPO/$d.old"
    mv "$STAGE/$d" "$REPO/$d"
    rm -rf "$REPO/$d.old"
done
gone=0
while IFS= read -r f; do
    rel=${f#"$REPO/pool/"}
    grep -qxF "$rel" <<< "$KEPT" && continue
    rm -f "$f"
    gone=$((gone + 1))
done < <(find "$REPO/pool" -name '*.rpg')
find "$REPO/pool" -mindepth 1 -type d -empty -delete
msg="publish-apps: published in $REPO: $n packages (${#NEW[@]} new): $(find "$REPO/pool" -name '*.rpg' -printf '%f\n' | sed 's/_x86_64\.rpg$//; s/_/ /' | sort | paste -sd, - | sed 's/,/, /g')"
[ "$gone" -gt 0 ] && msg="$msg; $gone files of older versions removed"
echo "$msg"
