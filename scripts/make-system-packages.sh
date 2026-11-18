#!/bin/bash
# The system's packages (package step E3, docs/PACKAGES.md): the system
# is image-based, so its packages carry the kernel file (with its boot
# image: /bin, /usr, every program, read-only) and say which edition:
#
#   rustica-system-VERSION   /boot/efi/EFI/Rustica/rustux-VERSION.efi
#                            (Section: images; versions sit side by side)
#   rustica-system           depends on rustica-system-VERSION and rboot
#   rboot                    /boot/efi/EFI/Rustica/rboot.efi
#   rustica-server           the edition: the text login by default
#   rustica-desktop          the edition: the graphical login by default
#
# The image and edition packages run `rpg boot update` after installing
# and removing, which writes rboot's entries.
#
#   scripts/make-system-packages.sh OUTDIR [VERSION]
#
# VERSION defaults to 0.1.N, N the commits so far. Uses rpg built for
# this computer ($RPG, else built here with cargo), the kernel from
# target/x86_64-unknown-uefi/release/rustux.efi ($KERNEL) and rboot from
# rboot/target/x86_64-unknown-uefi/release/rboot.efi ($RBOOT_EFI).
set -eu
cd "$(dirname "$0")/.."
OUT=${1:?usage: scripts/make-system-packages.sh OUTDIR [VERSION]}
V=${2:-0.1.$(git rev-list --count HEAD 2>/dev/null || echo 0)}
KERNEL=${KERNEL:-target/x86_64-unknown-uefi/release/rustux.efi}
RBOOT_EFI=${RBOOT_EFI:-rboot/target/x86_64-unknown-uefi/release/rboot.efi}
[ -f "$KERNEL" ] || { echo "no kernel at $KERNEL (build it first)"; exit 2; }
[ -f "$RBOOT_EFI" ] || { echo "no rboot at $RBOOT_EFI (build it first)"; exit 2; }
if [ -z "${RPG:-}" ]; then
    HOSTT=$(rustc -vV | sed -n 's/^host: //p')
    (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$PWD/../../build/rpg-host")
    RPG=$PWD/userspace/build/rpg-host/$HOSTT/release/rpg
fi
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
BOOT_UPDATE='rpg --root "${RPG_ROOT:-/}" boot update'

pkg() { # dir: RPG/control written, files in place
    "$RPG" build "$1" -o "$OUT/$2" > /dev/null
    echo "  $2"
}

echo "System packages $V in $OUT:"
d=$T/image
mkdir -p "$d/RPG" "$d/boot/efi/EFI/Rustica"
cp "$KERNEL" "$d/boot/efi/EFI/Rustica/rustux-$V.efi"
chmod 644 "$d/boot/efi/EFI/Rustica/rustux-$V.efi"
cat > "$d/RPG/control" <<EOF
Package: rustica-system-$V
Version: $V
Architecture: x86_64
Section: images
Maintainer: The Rustux Authors
Description: the Rustica system image $V
 The kernel with its boot image: every program, /bin and /usr,
 read-only. Images sit side by side; the newest boots, the previous one
 is in rboot's menu.
EOF
printf '%s\n' "$BOOT_UPDATE" > "$d/RPG/postinst"
printf '%s\n' "$BOOT_UPDATE" > "$d/RPG/postrm"
pkg "$d" "rustica-system-${V}_${V}_x86_64.rpg"

d=$T/rboot
mkdir -p "$d/RPG" "$d/boot/efi/EFI/Rustica"
cp "$RBOOT_EFI" "$d/boot/efi/EFI/Rustica/rboot.efi"
chmod 644 "$d/boot/efi/EFI/Rustica/rboot.efi"
cat > "$d/RPG/control" <<EOF
Package: rboot
Version: $V
Architecture: x86_64
Section: boot
Maintainer: The Rustux Authors
Description: Rustica's boot manager
 The program the firmware starts (\\EFI\\Rustica\\rboot.efi): it shows the
 systems on the disks and starts the chosen one (docs/BOOT.md).
EOF
pkg "$d" "rboot_${V}_x86_64.rpg"

d=$T/meta
mkdir -p "$d/RPG"
cat > "$d/RPG/control" <<EOF
Package: rustica-system
Version: $V
Architecture: x86_64
Section: images
Depends: rustica-system-$V (= $V), rboot (>= $V)
Maintainer: The Rustux Authors
Description: the newest Rustica system image
 Depends on the newest image package: rpg upgrade brings each new image.
EOF
pkg "$d" "rustica-system_${V}_x86_64.rpg"

for ed in server desktop; do
    other=desktop
    [ $ed = desktop ] && other=server
    d=$T/$ed
    mkdir -p "$d/RPG" "$d/var/lib/rustica"
    echo $ed > "$d/var/lib/rustica/edition"
    if [ $ed = server ]; then what="the text login by default"; else what="the graphical login (the desktop) by default"; fi
    cat > "$d/RPG/control" <<EOF
Package: rustica-$ed
Version: $V
Architecture: all
Section: editions
Depends: rustica-system
Conflicts: rustica-$other
Maintainer: The Rustux Authors
Description: Rustica's ${ed} edition
 The same system image, $what.
EOF
    printf '%s\n' "$BOOT_UPDATE" > "$d/RPG/postinst"
    pkg "$d" "rustica-${ed}_${V}_all.rpg"
done
