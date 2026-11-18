#!/bin/bash
# The publishing test (package step E5, docs/RELEASES.md), on this computer
# only: scripts/publish-repo.sh into a web root of its own, with a test key.
#   - a key inside the web root, and a key that is not the trusted one,
#     are refused, nothing published;
#   - 0.1.1 published; a client (rpg --root, only the public key) reads it
#     and installs the system from it; publishing 0.1.1 again does nothing;
#   - a pool file left by a run that failed (never in the index) does not
#     block the next one;
#   - 0.1.2-0.1.4 published: three versions kept, 0.1.1's files gone; the
#     client's rpg upgrade brings 0.1.4 (and keeps two images);
#   - a package changed on the server is refused by the client.
# Needs openssl, cargo, the kernel and rboot built.
set -u
cd "$(dirname "$0")/.."
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
fail=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fail=1; }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
(cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host") || { echo "rpg did not build"; exit 2; }
export RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
export PATH=$(dirname "$RPG"):$PATH
openssl genpkey -algorithm ed25519 -out "$W/test.key" 2>/dev/null; chmod 600 "$W/test.key"
openssl genpkey -algorithm ed25519 -out "$W/other.key" 2>/dev/null
echo "rpg-ed25519 $(openssl pkey -in "$W/test.key" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n') rpg-test" > "$W/archive-key"
R=$W/www/html/download/rustica
mkdir -p "$W/www/html/keys"; cp "$W/test.key" "$W/www/html/keys/"
cp "$W/archive-key" "$W/test.pub"
# (the public key's file name need not end in .pub)
pub() { scripts/publish-repo.sh --key "$1" --pub "$W/archive-key" --version "$2" "$R" > "$W/out" 2>&1; }

pub "$W/www/html/keys/test.key" 0.1.1; check 'grep -q "inside the web root" "$W/out" && [ ! -d "$R/dists" ]' "a key inside the web root is refused"
pub "$W/other.key" 0.1.1; check 'grep -q "is not the archive key" "$W/out" && [ ! -d "$R/dists" ]' "a key that is not the trusted one is refused"
pub "$W/test.key" 0.1.1; check 'grep -q "0.1.1 published in .*: 5 packages, versions 0.1.1$" "$W/out"' "0.1.1 published"; cat "$W/out" | grep -v '^publish-repo: 0.1.1 published' | head -5
# A client with only the public key, the server edition from the repository
C=$W/client
mkdir -p "$C/etc/rpg/keys" "$C/etc/rustica" "$C/boot/efi"
cp "$W/test.pub" "$C/etc/rpg/keys/"
echo "file://$R  redfox  main" > "$C/etc/rpg/sources.list"
echo "options root=UUID=test quiet" > "$C/etc/rustica/boot.conf"
rpg --root "$C" update > "$W/out" 2>&1; check 'grep -q "redfox: 5 packages" "$W/out"' "the client reads the signed index"
rpg --root "$C" install -y rustica-server > "$W/out" 2>&1; check 'rpg --root "$C" list --installed 2>/dev/null | grep -q "rustica-system-0.1.1 "' "the client installs the system from it"
pub "$W/test.key" 0.1.1; check 'grep -q "published already" "$W/out"' "publishing 0.1.1 again does nothing"
stray="$R/pool/main/r/rboot/rboot_0.1.2_x86_64.rpg"; echo "left by a run that failed" > "$stray"
pub "$W/test.key" 0.1.2; check 'grep -q "0.1.2 published" "$W/out" && ! grep -q "left by a run" "$stray"' "a file a failed run left in the pool does not block publishing"; grep -v "0.1.2 published" "$W/out" | head -3
for v in 0.1.3 0.1.4; do pub "$W/test.key" $v || cat "$W/out"; done
check 'grep -q "0.1.4 published in .*: 15 packages, versions 0.1.2,0.1.3,0.1.4; 5 package files of older versions removed" "$W/out"' "three versions kept, 0.1.1's files gone"; tail -1 "$W/out"
check '[ -z "$(find "$R/pool" -name "*_0.1.1_*")" ] && [ ! -e "$R/.staging" ] && [ ! -e "$R/dists.old" ]' "nothing left behind (0.1.1's files, the staging folder)"
rpg --root "$C" update > "$W/out" 2>&1; check 'grep -q "redfox: 15 packages" "$W/out"' "the client sees the three versions"
rpg --root "$C" upgrade -y > "$W/out" 2>&1; check 'grep -q "rustica-system: 0.1.1 -> 0.1.4" "$W/out" && rpg --root "$C" list --installed 2>/dev/null | grep -q "rustica-system-0.1.4 "' "rpg upgrade brings 0.1.4"
check 'grep -q "two system images are kept" "$W/out" || [ "$(rpg --root "$C" list --installed | grep -c "^rustica-system-")" -le 2 ]' "the client keeps two images"
# A package changed on the server after signing
f=$(find "$R/pool" -name 'rboot_0.1.4_*.rpg'); printf 'x' >> "$f"
rm -rf "$C/var/cache/rpg"
rpg --root "$C" download rboot > "$W/out" 2>&1; check 'grep -qi "differs\|refused\|SHA-256" "$W/out"' "a package changed on the server is refused"
[ $fail -eq 0 ] && echo "run-publish-test: all checks passed" || echo "run-publish-test: some checks failed"
exit $fail
