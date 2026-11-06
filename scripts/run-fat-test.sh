#!/bin/bash
# The FAT test against the host's tools (step 14, docs/TESTS.md): boots
# Rustux in QEMU with two FAT disks, has it run
# /usr/share/fat/host-check.sh (a large copy onto a FAT32 made by the
# host's mkfs.fat; mkfs.fat on an empty disk and files on it), then checks
# both disks with the host's fsck.fat and reads every file back with
# mtools. Exit status 0 if all is well.
#
#   scripts/run-fat-test.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools, dosfstools (mkfs.fat,
# fsck.fat), socat, python3.
set -u
cd "$(dirname "$0")/.."
ROOT=$PWD
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no kernel at $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mcopy mformat mkfs.fat fsck.fat socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done
# mtools needs a UTF-8 locale for names outside ASCII
export LC_ALL=C.UTF-8
W=$(mktemp -d)
trap 'kill $Q 2>/dev/null; rm -rf "$W"' EXIT
dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
# Disk A: FAT32 from the host, with files; disk B: zeros
dd if=/dev/zero of="$W/a.img" bs=1M count=256 status=none
mkfs.fat -F 32 -n LINUXA "$W/a.img" > /dev/null
echo "hello from the host" > "$W/hello.txt"
mmd -i "$W/a.img" ::/from-linux
mcopy -i "$W/a.img" "$W/hello.txt" ::/from-linux/hello.txt
mcopy -i "$W/a.img" "$W/hello.txt" ::/from-linux/delete-me.txt
dd if=/dev/zero of="$W/b.img" bs=1M count=128 status=none
qemu-system-x86_64 -machine q35 -m 1G -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -drive file="$W/a.img",if=none,id=a,format=raw -device virtio-blk-pci,drive=a \
    -drive file="$W/b.img",if=none,id=b,format=raw -device virtio-blk-pci,drive=b \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    >/dev/null 2>&1 &
Q=$!
for _ in $(seq 120); do
    grep -aq "rustux.*:.*/root.* > " "$W/console.log" 2>/dev/null && break
    sleep 1
done
keys() {
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','/':'slash','.':'dot'}
print(' '.join('shift-'+c.lower() if c.isupper() else c if c.isalnum() else m[c] for c in sys.argv[1]))
PY
}
for k in $(keys $'sh /usr/share/fat/host-check.sh\n'); do
    echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" > /dev/null
    sleep 0.1
done
for _ in $(seq 600); do
    grep -aq "FATTEST done\|rustux.*> $" "$W/console.log" 2>/dev/null && grep -aq "FATTEST done" "$W/console.log" && break
    sleep 1
done
sleep 2
kill $Q 2>/dev/null; wait $Q 2>/dev/null
sed 's/\x1b\[[0-9;?]*[a-zA-Z]//g; s/\r$//' "$W/console.log" | sed -n '/FATTEST start/,/FATTEST done/p'
fail=0
grep -aq "FATTEST done" "$W/console.log" || { echo "FAIL: the script in Rustux did not finish"; fail=1; }
for d in a b; do
    if fsck.fat -n "$W/$d.img" > "$W/fsck-$d.txt" 2>&1; then
        echo "host fsck.fat disk $d: $(tail -1 "$W/fsck-$d.txt")"
    else
        echo "FAIL: host fsck.fat disk $d:"; cat "$W/fsck-$d.txt"; fail=1
    fi
done
seq 1 2000000 > "$W/seq.txt"
same() { # image, path on it, local file
    rm -f "$W/got"
    mcopy -n -i "$W/$1.img" "::$2" "$W/got" 2>/dev/null && cmp -s "$W/got" "$3" || { echo "FAIL: $1:$2 differs"; fail=1; }
}
same a "/Large Copy/numbers 1.txt" "$W/seq.txt"
same a "/from-linux/Zahlen – drei.txt" "$W/seq.txt"
same a /from-linux/hello.txt "$W/hello.txt"
mtype -i "$W/a.img" ::/from-linux/rustux.txt | grep -q "written by Rustux" || { echo "FAIL: a:/from-linux/rustux.txt"; fail=1; }
mdir -i "$W/a.img" "::/Large Copy/numbers 2.txt" > /dev/null 2>&1 && { echo "FAIL: a: a deleted file is there"; fail=1; }
mdir -i "$W/a.img" ::/from-linux/delete-me.txt > /dev/null 2>&1 && { echo "FAIL: a: a deleted file is there"; fail=1; }
same b /numbers.txt "$W/seq.txt"
mtype -i "$W/b.img" "::/EFI/Rustux Files/deeper/README.txt" | grep -q "made on Rustux" || { echo "FAIL: b: README.txt"; fail=1; }
for name in coreutils tools shell fsd fatfs mount mkfs.fat fsck.fat devmgr lsblk; do
    same a "/Programs from Rustux/$name" "userspace/bin/$name"
    same b "/bin/$name" "userspace/bin/$name"
done
for i in 1 3 5 6; do same b "/fill/$i" "$W/seq.txt"; done
mlabel -i "$W/b.img" -s :: 2>/dev/null | grep -q RUSTUXB || { echo "FAIL: b: the label"; fail=1; }
[ $fail = 0 ] && echo "run-fat-test: all checks passed"
exit $fail
