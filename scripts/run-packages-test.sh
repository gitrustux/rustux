#!/bin/bash
# The packages test (phase E step E1, docs/PACKAGES.md, docs/TESTS.md).
# Builds rpg for the host and a package with it (hello 1.0-1), puts it on
# a FAT disk (RPGTEST), then boots Rustux in QEMU three times with the
# root filesystem on ext4 (root=UUID=...), autologin as root:
#   1. /usr/share/rpg/tests/boot1.sh: the host's package installed, its
#      files rpg's (owner, files, verify, the postinst); packages built in
#      Rustux: an upgrade (a changed configuration file kept, the new one
#      beside it as .rpg-new; files gone and new), a file of another
#      package and an unmet dependency refused, removal and --purge, a
#      package with a file in the read-only boot image (/usr) refused
#      with nothing left, changes refused to a user; then an upgrade of 300 files paused
#      half-way through its renames, and QEMU killed (a power cut);
#   2. boot2.sh: rpg fix finishes that upgrade (every file version 2,
#      nothing left over); an upgrade paused while its files are being
#      written, and QEMU killed again;
#   3. boot3.sh: rpg fix undoes that one (version 2 untouched); removal;
#      a clean poweroff.
# Then the host's e2fsck -f finds the disk clean. Exit status 0 if all
# is well.
#
#   scripts/run-packages-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, cargo (rpg for the host), mtools,
# dosfstools, e2fsprogs, socat, python3.
set -u
cd "$(dirname "$0")/.."
ROOT=$PWD
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
export PATH=$PATH:/sbin:/usr/sbin
for t in qemu-system-x86_64 cargo mformat mmd mcopy mkfs.fat mkfs.ext4 e2fsck socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
Q=0
ROOT_UUID=33333333-4444-4555-8666-777777777777
fail=0

echo "== rpg for the host, and hello 1.0-1 built with it"
HOSTT=$(rustc -vV | sed -n 's/^host: //p')
if ! (cd userspace/rust/rpg && cargo build -q --release --target "$HOSTT" --target-dir "$W/cargo"); then
    echo "FAIL: rpg did not build for the host"; exit 1
fi
RPG="$W/cargo/$HOSTT/release/rpg"
H="$W/hello"
mkdir -p "$H/RPG" "$H/etc/hello.d" "$H/opt/hello/share" "$H/opt/hello/bin"
printf 'Package: hello\nVersion: 1.0-1\nArchitecture: all\nMaintainer: Rustux tests\nDescription: says hello\n A test package for rpg (step E1).\n' > "$H/RPG/control"
printf 'echo "postinst $*" > "$RPG_ROOT/var/tmp-hello"\n' > "$H/RPG/postinst"
printf 'greeting=hello\n' > "$H/etc/hello.conf"
printf 'colour=red\n' > "$H/etc/hello.d/defaults"
printf 'Hello, version 1\n' > "$H/opt/hello/share/README"
printf 'old file\n' > "$H/opt/hello/share/OLD"
printf 'echo hello from version 1\n' > "$H/opt/hello/bin/hello-rpg"
chmod 755 "$H/opt/hello/bin/hello-rpg"
ln -s hello-rpg "$H/opt/hello/bin/hi"
(cd "$W" && "$RPG" build hello) || { echo "FAIL: rpg build on the host"; exit 1; }
tar tzf "$W/hello_1.0-1_all.rpg" | grep -qx "data/opt/hello/bin/hello-rpg" || { echo "FAIL: the host's tar does not read the package"; fail=1; }

dd if=/dev/zero of="$W/p.img" bs=1M count=16 status=none
mkfs.fat -n RPGTEST "$W/p.img" > /dev/null
mcopy -i "$W/p.img" "$W/hello_1.0-1_all.rpg" ::/
truncate -s 256M "$W/root.img"
mkfs.ext4 -q -L rootfs -U $ROOT_UUID "$W/root.img"
dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
echo "root=UUID=$ROOT_UUID" > "$W/cmdline.txt"
mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt

keys() {
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','=':'equal','[':'bracket_left',']':'bracket_right',';':'semicolon',"'":'apostrophe','`':'grave_accent','\\':'backslash',',':'comma','.':'dot','/':'slash'}
sh={'!':'1','@':'2','#':'3','$':'4','%':'5','^':'6','&':'7','*':'8','(':'9',')':'0','_':'minus','+':'equal','{':'bracket_left','}':'bracket_right',':':'semicolon','"':'apostrophe','~':'grave_accent','|':'backslash','<':'comma','>':'dot','?':'slash'}
out=[]
for c in sys.argv[1]:
    if c.isupper(): out.append('shift-'+c.lower())
    elif c.isalnum(): out.append(c)
    elif c in m: out.append(m[c])
    elif c in sh: out.append('shift-'+m.get(sh[c],sh[c]))
print(' '.join(out))
PY
}
type_text() {
    for k in $(keys "$1"); do
        echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
        sleep 0.08
    done
}
log() { [ -f "$W/console.log" ] || return 0; sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g; s/\r$//' "$W/console.log"; }
wait_for() { # pattern, seconds
    for _ in $(seq $(( $2 * 2 ))); do
        log | grep -aq -- "$1" && return 0
        sleep 0.5
    done
    echo "FAIL: did not see \"$1\" in time"; fail=1
    return 1
}
start_qemu() {
    rm -f "$W/console.log" "$W/mon"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
        -drive file="$W/root.img",if=none,id=r,format=raw -device virtio-blk-pci,drive=r \
        -drive file="$W/p.img",if=none,id=p,format=raw -device virtio-blk-pci,drive=p \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        >/dev/null 2>&1 &
    Q=$!
}
# A power cut: QEMU killed, with whatever the disk has
power_cut() {
    kill -9 $Q 2>/dev/null
    wait $Q 2>/dev/null
}
wait_end() { # seconds
    for _ in $(seq $1); do
        kill -0 $Q 2>/dev/null || return 0
        sleep 1
    done
    echo "FAIL: QEMU did not power off"; fail=1
    power_cut
}
section() { log | sed -n "/PKGTEST $1/,\$p"; }
expect() { # text that must be in the section's output
    printf '%s\n' "$OUT" | grep -aqF -- "$1" || { echo "FAIL: expected: $1"; fail=1; }
}
refuse() { # text that must not be there
    printf '%s\n' "$OUT" | grep -aqF -- "$1" && { echo "FAIL: not expected: $1"; fail=1; }
}
run_boot() { # script, the marker to wait for, seconds
    start_qemu
    if wait_for "rustux.*:.*/root.* > " 120; then
        type_text "mkdir -p /tmp/p; mount \$(blkid -L RPGTEST) /tmp/p; sh /usr/share/rpg/tests/$1"$'\n'
        wait_for "$2" "$3"
    fi
}

echo "== boot 1: install, upgrade, remove; a power cut during the renames"
run_boot boot1.sh "rpg: test pause (rename 150)" 600
sleep 1
power_cut
OUT=$(section start)
printf '%s\n' "$OUT" | sed -n '/PKGTEST start/,/PKGTEST cut/p'
expect "rpg 0.1.0 (step E1"
expect "No packages are installed."
expect "hello 1.0-1: installed (6 files)"
expect "postinst install"
expect "hello: /etc/hello.conf"
expect "hello: /opt/hello/bin/hi"
expect "hello from version 1"
expect "hello: 6 files as installed"
expect "hello: 1.0-1 -> 1.1-1 (6 files)"
expect "/etc/hello.conf: changed here, so kept; the package's new one is /etc/hello.conf.rpg-new"
expect "postinst upgrade 1.0-1"
expect "greeting=hi there"
expect "loud=no"
expect "colour=blue"
expect "changed  /etc/hello.conf  (configuration file)"
expect "/opt/hello/share/OLD': No such file or directory"
expect "hello from version 2"
expect "rpg: other: /opt/hello/share/README belongs to hello too; nothing was changed"
expect "Hello, version 2"
expect "rpg: needs-hello 1 needs hello (>= 2.0); hello 1.1-1 is installed"
expect "needs-hello 1: installed (1 files)"
expect "rpg: needs-hello 1 needs hello (>= 1.1); remove it too (rpg remove hello needs-hello)"
expect "hello 1.1-1: removed; its configuration files are kept (rpg remove --purge hello removes them)"
expect "needs-hello 1: removed"
expect "hello: its configuration files removed"
expect "(on Rustux, /bin, /usr and /boot are the boot image, read-only, until step E3: packages go in /opt, /etc and /var); nothing was changed"
refuse "rpg: the install of in-usr"
expect "rpg: changing the system's packages needs root (sudo rpg ...)"
expect "big 1: installed (300 files)"

echo "== boot 2: rpg fix finishes it; a power cut while staging"
run_boot boot2.sh "rpg: test pause (stage 100)" 300
sleep 1
power_cut
OUT=$(section boot2)
printf '%s\n' "$OUT" | sed -n '/PKGTEST boot2/,/PKGTEST cut/p'
expect "rpg: the install of big 2 was interrupted; rpg fix finishes it"
expect "rpg: finishing the interrupted install of big 2"
expect "big: 300 files as installed"
expect "version 2 files: 300"
expect "rpg-tmp files: 0"
refuse "journal"

echo "== boot 3: rpg fix undoes that one; removal; poweroff"
run_boot boot3.sh "PKGTEST done" 300
wait_end 120
OUT=$(section boot3)
printf '%s\n' "$OUT" | sed -n '/PKGTEST boot3/,/PKGTEST done/p'
expect "rpg-tmp files: 100"
expect "rpg: undid the interrupted install of big 3: big stays at 2"
expect "big: 300 files as installed"
expect "version 2 files: 300"
expect "rpg-tmp files: 0"
expect "big 2: removed"
expect "No packages are installed."
expect "rpg: nothing to finish"

if e2fsck -fn "$W/root.img" > "$W/fsck.txt" 2>&1; then
    echo "host e2fsck -f: $(tail -1 "$W/fsck.txt")"
else
    echo "FAIL: host e2fsck -f:"; sed -n '2,30p' "$W/fsck.txt"; fail=1
fi

if [ $fail -eq 0 ]; then echo "run-packages-test: all checks passed"; else echo "run-packages-test: FAILED"; fi
exit $fail
