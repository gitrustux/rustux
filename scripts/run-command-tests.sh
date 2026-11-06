#!/bin/bash
# Run the command test suite (userspace `runtests`, checklist step 11) in
# QEMU: boots the kernel, types `runtests` at the shell through QEMU's
# monitor, and waits for the summary on the debug console. Exit status:
# 0 if every test passed.
#
#   scripts/run-command-tests.sh [KERNEL.efi]
#
# Needs qemu-system-x86_64, OVMF, mtools (mformat/mcopy), socat, python3.
# Default kernel: target/x86_64-unknown-uefi/release/rustux.efi (build.sh).
set -u
cd "$(dirname "$0")/.."
EFI=${1:-target/x86_64-unknown-uefi/release/rustux.efi}
[ -f "$EFI" ] || { echo "no kernel at $EFI (build it first)"; exit 2; }
OVMF=""
for p in /usr/share/ovmf/OVMF.fd /usr/share/qemu/OVMF.fd /usr/share/edk2-ovmf/x64/OVMF_CODE.fd /usr/share/OVMF/OVMF_CODE.fd; do
    [ -f "$p" ] && OVMF=$p && break
done
[ -n "$OVMF" ] || { echo "OVMF firmware not found"; exit 2; }
for t in qemu-system-x86_64 mcopy mformat socat python3; do
    command -v $t >/dev/null || { echo "$t is needed"; exit 2; }
done

W=$(mktemp -d)
# KEEP_LOG=FILE keeps the console log
trap 'kill $Q 2>/dev/null; [ -n "${KEEP_LOG:-}" ] && cp "$W/console.log" "$KEEP_LOG"; rm -rf "$W"' EXIT
dd if=/dev/zero of="$W/boot.img" bs=1M count=64 status=none
# The disk tests' disk (disks.sh), as virtio-blk and as NVMe; snapshot=on
# keeps writes (and the firmware's NvVars file) out of the image
python3 "$(dirname "$0")/make-test-disk.py" make "$W/disk.img" > /dev/null
mformat -i "$W/boot.img" -F ::
mmd -i "$W/boot.img" ::/EFI ::/EFI/BOOT
mcopy -i "$W/boot.img" "$EFI" ::/EFI/BOOT/BOOTX64.EFI
qemu-system-x86_64 -machine q35 -m 512M -smp 2 -bios "$OVMF" -drive file="$W/boot.img",format=raw \
    -device edu,dma_mask=0xffffffff \
    -drive file="$W/disk.img",if=none,id=vd,format=raw,snapshot=on,file.locking=off -device virtio-blk-pci,drive=vd \
    -drive file="$W/disk.img",if=none,id=nv,format=raw,snapshot=on,file.locking=off -device nvme,drive=nv,serial=rustux \
    -device qemu-xhci,id=xhci -drive file="$W/disk.img",if=none,id=us,format=raw,snapshot=on,file.locking=off \
    -device usb-storage,bus=xhci.0,drive=us,removable=on \
    -display none -debugcon file:"$W/console.log" -no-reboot -monitor unix:"$W/mon",server,nowait \
    >/dev/null 2>&1 &
Q=$!

# Wait for the shell's prompt
for _ in $(seq 120); do
    grep -aq "rustux.*:.*/root.* > " "$W/console.log" 2>/dev/null && break
    sleep 1
done
keys() { # text -> QEMU key names
    python3 - "$1" <<'PY'
import sys
m={' ':'spc','\n':'ret','-':'minus','=':'equal','/':'slash','.':'dot',';':'semicolon'}
out=[]
for c in sys.argv[1]:
    if c.isupper(): out.append('shift-'+c.lower())
    elif c.isalnum(): out.append(c)
    else: out.append(m[c])
print(' '.join(out))
PY
}
for k in $(keys $'runtests -v\n'); do
    echo "sendkey $k" | socat - UNIX-CONNECT:"$W/mon" >/dev/null
    sleep 0.1
done
for _ in $(seq 600); do
    grep -aq "runtests: [0-9]* passed" "$W/console.log" && break
    sleep 1
done
sed -n '/runtests -v/,/runtests: [0-9]* passed/p' "$W/console.log" | tr -d '\r'
line=$(grep -a "runtests: [0-9]* passed" "$W/console.log" | tail -1)
[ -n "$line" ] || { echo "no result (timed out)"; exit 1; }
case $line in *", 0 failed"*) exit 0 ;; *) exit 1 ;; esac
