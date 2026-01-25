#!/bin/bash
set -e

TIMESTAMP=$(date +%s%N)
IMG="/tmp/rustux-${TIMESTAMP}.img"
LOG="/tmp/rustux-debug-${TIMESTAMP}.log"

echo "Creating bootable image..."
rm -rf /tmp/rustux-efi-tmp
mkdir -p /tmp/rustux-efi-tmp/EFI/BOOT
cp target/x86_64-unknown-uefi/release/rustux.efi /tmp/rustux-efi-tmp/EFI/BOOT/BOOTX64.EFI

dd if=/dev/zero of="${IMG}" bs=1M count=64 status=none
mkfs.fat -F 32 "${IMG}" >/dev/null 2>&1
mcopy -i "${IMG}" -s /tmp/rustux-efi-tmp/EFI :: 2>/dev/null

echo "Running QEMU (log: ${LOG})..."
timeout 30s qemu-system-x86_64 \
    -bios /usr/share/ovmf/OVMF.fd \
    -drive format=raw,file="${IMG}" \
    -debugcon file:"${LOG}" \
    -display none \
    -no-reboot \
    -no-shutdown 2>&1 | head -20 || true

echo ""
echo "=== Segment 2 Debug Output ==="
cat "${LOG}" | grep -A 30 "Segment 2" | grep "\[MAP\]"

echo ""
echo "=== Last 50 MAP messages ==="
cat "${LOG}" | grep "\[MAP\]" | tail -50

rm -f "${IMG}"
rm -rf /tmp/rustux-efi-tmp
echo ""
echo "Full log: ${LOG}"
