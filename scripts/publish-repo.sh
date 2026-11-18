#!/bin/bash
# Publish the system's packages as the signed repository (package step E5,
# docs/RELEASES.md): what https://rustux.com/download/rustica serves.
#
#   scripts/publish-repo.sh --key KEYFILE [--pub PUBFILE] [--keep N]
#                           [--version V] REPODIR
#
# 1. The key: an Ed25519 private key (OpenSSL's PEM) whose public half must
#    be the archive key the image trusts (files/etc/rpg/keys/
#    rustux-archive-2026.pub, or --pub, for tests). It must not be inside
#    the web root (the folder two above REPODIR) or under /var/www.
# 2. The packages for VERSION (default 0.1.N, N the commits so far:
#    scripts/make-system-packages.sh, from the built kernel and rboot) are
#    made in a staging folder beside REPODIR's pool. A version the signed
#    index lists already is left as it is (published packages never
#    change): nothing to do.
# 3. A new index of the newest N versions (default 3) is made in a staging
#    folder beside REPODIR, Release signed with the key (openssl pkeyutl),
#    then checked as a client would: rpg update, with only the public key,
#    from that folder.
# 4. Only then the new packages go into the pool, the new dists/ replaces
#    the old one (two renames), and pool files of older versions go. A
#    failed run leaves the repository as it was.
#
# Needs openssl 3, cargo (rpg for this computer, unless $RPG), the kernel and
# rboot built (as make-system-packages.sh says).
set -euo pipefail
cd "$(dirname "$0")/.."
KEY="" PUB=files/etc/rpg/keys/rustux-archive-2026.pub KEEP=3 V="" REPO=""
while [ $# -gt 0 ]; do
    case $1 in
        --key) KEY=$2; shift 2 ;;
        --pub) PUB=$2; shift 2 ;;
        --keep) KEEP=$2; shift 2 ;;
        --version) V=$2; shift 2 ;;
        -*) echo "publish-repo: $1?"; exit 2 ;;
        *) REPO=$1; shift ;;
    esac
done
[ -n "$KEY" ] && [ -n "$REPO" ] || { echo "usage: scripts/publish-repo.sh --key KEYFILE [--pub PUBFILE] [--keep N] [--version V] REPODIR"; exit 2; }
[ "$KEEP" -ge 2 ] 2>/dev/null || { echo "publish-repo: --keep needs 2 or more (clients may still want the one before)"; exit 2; }
V=${V:-0.1.$(git rev-list --count HEAD)}
SUITE=redfox
die() { echo "publish-repo: $*"; exit 1; }
command -v openssl >/dev/null || die "openssl is needed"

# --- 1. the key
[ -r "$KEY" ] || die "$KEY: cannot read the key"
mkdir -p "$REPO"
REPO=$(cd "$REPO" && pwd)
KEYREAL=$(realpath "$KEY")
WEBROOT=$(dirname "$(dirname "$REPO")")
case $KEYREAL in
    /var/www/*|"$WEBROOT"/*) die "$KEYREAL is inside the web root: keep the archive key out of it (and out of git)" ;;
esac
if [ -n "$(find "$KEYREAL" -perm /077 2>/dev/null)" ]; then
    echo "publish-repo: warning: $KEYREAL can be read by others than its owner (chmod 600 it)"
fi
want=$(awk '$1 == "rpg-ed25519" { print $2 }' "$PUB")
have=$(openssl pkey -in "$KEY" -pubout -outform DER 2>/dev/null | tail -c 32 | od -An -tx1 | tr -d ' \n')
[ -n "$have" ] || die "$KEY: not an Ed25519 private key openssl can read"
[ "$have" = "$want" ] || die "$KEY is not the archive key the image trusts ($PUB): refused, nothing changed"

# --- rpg for this computer
if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host")
    RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
fi
export RPG

# --- 2. the packages: into the staging folder only (the pool gets them
# once everything is checked, so a failed run leaves nothing behind)
T=$(mktemp -d)
STAGE="$REPO/.staging"
trap 'rm -rf "$T" "$STAGE"' EXIT
# What the signed index lists: published, never to change
indexed() { find "$REPO/dists/$SUITE" -name Packages.gz -exec zcat {} + 2>/dev/null | sed -n 's/^Filename: //p'; }
if indexed | grep -q "/rustica-system-${V}_${V}_"; then
    echo "publish-repo: $V is published already (published packages never change): nothing to do"
    exit 0
fi
rm -rf "$STAGE"
mkdir -p "$STAGE/pool"
scripts/make-system-packages.sh "$T/new" "$V" > /dev/null
NEW=()
for f in "$T"/new/*.rpg; do
    rel=$("$RPG" pool-path "$f")
    if indexed | grep -qxF "$rel" && ! cmp -s "$f" "$REPO/$rel"; then
        die "$rel is published with other contents: a version is never published twice"
    fi
    mkdir -p "$STAGE/$(dirname "$rel")"
    install -m 644 "$f" "$STAGE/$rel"
    NEW+=("$rel")
done

# --- 3. the newest KEEP versions, indexed and signed in the staging folder
# (NAME_VERSION_ARCH.rpg: names have no "_", architectures may: x86_64)
versions=$( { find "$REPO/pool" -name 'rustica-system-*_*_*.rpg' -printf '%f\n' 2>/dev/null; echo "x_${V}_x"; } | cut -d_ -f2 | sort -u -V | tail -n "$KEEP")
while IFS= read -r f; do
    ver=$(basename "$f" | cut -d_ -f2)
    grep -qxF "$ver" <<< "$versions" || continue
    rel=${f#"$REPO"/}
    [ -e "$STAGE/$rel" ] && continue
    mkdir -p "$STAGE/$(dirname "$rel")"
    ln "$f" "$STAGE/$rel" 2>/dev/null || cp "$f" "$STAGE/$rel"
done < <(find "$REPO/pool" -name '*.rpg' 2>/dev/null)
n=$(find "$STAGE/pool" -name '*.rpg' | wc -l)
"$RPG" index "$STAGE" --suite "$SUITE" > /dev/null
openssl pkeyutl -sign -inkey "$KEY" -rawin -in "$STAGE/dists/$SUITE/Release" -out "$STAGE/dists/$SUITE/Release.sig"
chmod -R a+rX "$STAGE/dists" "$STAGE/pool"
# As a client: only the public key, rpg update from the staging folder
C=$T/client
mkdir -p "$C/etc/rpg/keys"
cp "$PUB" "$C/etc/rpg/keys/archive.pub"
echo "file://$STAGE  $SUITE  main" > "$C/etc/rpg/sources.list"
out=$("$RPG" --root "$C" update 2>&1) || die "the new index does not pass rpg update (nothing published): $out"
grep -q "file://$STAGE $SUITE: $n packages" <<< "$out" || die "rpg update read something else than the $n packages (nothing published): $out"

# --- 4. the new packages into the pool, the new index in place, then
# old versions go
for rel in "${NEW[@]}"; do
    mkdir -p "$REPO/$(dirname "$rel")"
    install -m 644 "$STAGE/$rel" "$REPO/$rel"
done
rm -rf "$REPO/dists.old"
[ -d "$REPO/dists" ] && mv "$REPO/dists" "$REPO/dists.old"
mv "$STAGE/dists" "$REPO/dists"
rm -rf "$REPO/dists.old"
gone=0
while IFS= read -r f; do
    ver=$(basename "$f" | cut -d_ -f2)
    grep -qxF "$ver" <<< "$versions" && continue
    rm -f "$f"
    gone=$((gone + 1))
done < <(find "$REPO/pool" -name '*.rpg')
find "$REPO/pool" -mindepth 1 -type d -empty -delete
msg="publish-repo: $V published in $REPO: $n packages, versions $(echo $versions | tr ' ' ',')"
[ "$gone" -gt 0 ] && msg="$msg; $gone package files of older versions removed"
echo "$msg"
