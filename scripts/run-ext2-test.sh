#!/bin/bash
# The ext2 test against the host's tools (step 15, docs/TESTS.md). Boots
# Rustux in QEMU three times:
#   1. with root=UUID=... (in \EFI\BOOT\cmdline.txt): the root filesystem
#      on an ext2 disk made by the host; /usr/share/ext2/boot1.sh then
#      copies a lot onto a second ext2 disk made by the host's mkfs.ext2,
#      runs mkfs.ext2 on a third disk and fills it, writes files for the
#      next boot, and powers off;
#   2. the same: the files of boot 1 are there (boot2.sh);
#   3. no command line and no disks: Rustux runs from memory (boot3.sh).
# Then the host's e2fsck -f checks the three disks and debugfs reads the
# files back. Exit status 0 if all is well.
#
#   scripts/run-ext2-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, e2fsprogs (mkfs.ext2, e2fsck,
# debugfs), dosfstools, socat, python3.
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no kernel at $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
export PATH="$PATH:/sbin:/usr/sbin"
for t in qemu-system-x86_64 mcopy mformat mkfs.ext2 e2fsck debugfs mkfs.vfat socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
W=$(mktemp -d)
Q=0
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
ROOT_UUID=11111111-2222-4333-8444-555555555555

boot_image() { # with a command line, or none
    rm -f "$W/boot.img"
    dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
    mformat -i "$W/boot.img" -F ::
    mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
    mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
    if [ -n "$1" ]; then
        echo "$1" > "$W/cmdline.txt"
        mcopy -i "$W/boot.img" "$W/cmdline.txt" ::/EFI/BOOT/cmdline.txt
    fi
}

keys() {
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','/':'slash','.':'dot'}
print(' '.join('shift-'+c.lower() if c.isupper() else c if c.isalnum() else m[c] for c in sys.argv[1]))
PY
}

# Boot, type `sh SCRIPT`, wait until Rustux powers off (at most 10 min)
run() { # script, extra qemu args...
    local script=$1
    shift
    rm -f "$W/console.log" "$W/mon"
    qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw "$@" \
        -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
        >/dev/null 2>&1 &
    Q=$!
    for _ in $(seq 120); do
        grep -aq "rustux.*:.*/root.* > " "$W/console.log" 2>/dev/null && break
        sleep 1
    done
    for k in $(keys "sh $script"$'\n'); do
        echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
        sleep 0.1
    done
    for _ in $(seq 600); do
        kill -0 $Q 2>/dev/null || break
        sleep 1
    done
    kill $Q 2>/dev/null
    wait $Q 2>/dev/null
    sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g; s/\r$//' "$W/console.log" | sed -n '/EXT2TEST start/,/EXT2TEST done/p'
    grep -aq "EXT2TEST done" "$W/console.log" || { echo "FAIL: $script did not finish"; fail=1; }
}

fail=0
# The disks: the root (empty ext2), a Linux-made ext2 with files, a blank one
truncate -s 256M "$W/root.img"
mkfs.ext2 -q -L rootfs -U $ROOT_UUID "$W/root.img"
mkdir -p "$W/src/from-linux"
echo "hello from the host" > "$W/src/from-linux/hello.txt"
cp "$W/src/from-linux/hello.txt" "$W/src/from-linux/delete-me"
ln -s hello.txt "$W/src/from-linux/link"
ln -s /etc/hostname "$W/src/from-linux/abs"
ln "$W/src/from-linux/hello.txt" "$W/src/from-linux/hard"
truncate -s 30M "$W/src/from-linux/sparse"
echo end >> "$W/src/from-linux/sparse"
truncate -s 512M "$W/a.img"
mkfs.ext2 -q -L linuxa -d "$W/src" "$W/a.img"
truncate -s 128M "$W/b.img"
mkfs.vfat -n BLANKB "$W/b.img" > /dev/null
DISKS=(-drive file="$W/root.img",if=none,id=r,format=raw -device virtio-blk-pci,drive=r
       -drive file="$W/a.img",if=none,id=a,format=raw -device virtio-blk-pci,drive=a
       -drive file="$W/b.img",if=none,id=b,format=raw -device virtio-blk-pci,drive=b)

boot_image "root=UUID=$ROOT_UUID"
echo "== boot 1"
run /usr/share/ext2/boot1.sh "${DISKS[@]}"
echo "== boot 2"
run /usr/share/ext2/boot2.sh "${DISKS[@]}"
boot_image ""
echo "== boot 3 (no disks)"
run /usr/share/ext2/boot3.sh
sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g' "$W/console.log" | grep -aq "^tmpfs on / type tmpfs" || { echo "FAIL: boot 3 did not run from memory"; fail=1; }

for d in root a b; do
    if e2fsck -fn "$W/$d.img" > "$W/fsck-$d.txt" 2>&1; then
        echo "host e2fsck -f $d: $(tail -1 "$W/fsck-$d.txt")"
    else
        echo "FAIL: host e2fsck -f $d:"; cat "$W/fsck-$d.txt"; fail=1
    fi
    state=$(dumpe2fs -h "$W/$d.img" 2>/dev/null | sed -n 's/^Filesystem state: *//p')
    [ "$state" = clean ] || { echo "FAIL: $d is not marked clean ($state)"; fail=1; }
done
seq 1 2000000 > "$W/seq.txt"
same() { # image, path, local file
    debugfs -R "dump \"$2\" $W/got" "$W/$1.img" > /dev/null 2>&1
    cmp -s "$W/got" "$3" || { echo "FAIL: $1:$2 differs"; fail=1; }
    rm -f "$W/got"
}
same a "/Large Copy/numbers 1.txt" "$W/seq.txt"
same a /from-linux/moved.txt "$W/seq.txt"
same a /second-name "$W/seq.txt"
for p in coreutils tools shell fsd ext2fs fatfs mount devmgr; do
    same a /programs/$p userspace/bin/$p
    same b /$p userspace/bin/$p
done
for i in 1 3; do same b /fill/$i "$W/seq.txt"; done
n=$(debugfs -R "ls /many" "$W/a.img" 2>/dev/null | tr -s ' ' '\n' | grep -c '^file-')
[ "$n" = 489 ] || { echo "FAIL: a:/many has $n files, not 489"; fail=1; }
debugfs -R "cat /root/persist.txt" "$W/root.img" 2>/dev/null | grep -q "written in boot one" || { echo "FAIL: root:/root/persist.txt"; fail=1; }
debugfs -R "stat /from-linux/delete-me" "$W/a.img" 2>&1 | grep -q "not found" || { echo "FAIL: a: a removed file is still there"; fail=1; }
[ $fail = 0 ] && echo "run-ext2-test: all checks passed"
exit $fail
