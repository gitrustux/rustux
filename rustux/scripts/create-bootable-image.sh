#!/bin/bash
# Create a bootable disk image with the Rustux kernel

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "Creating bootable disk image..."

# Remove old image if exists
if [ -f "rustux.img" ]; then
    rm rustux.img
fi

# Create 64MB disk image
dd if=/dev/zero of=rustux.img bs=1M count=64 2>&1 | grep -v records | grep -v bytes

# Format as FAT32
mkfs.fat -F 32 rustux.img 2>&1 | tail -2

# Create EFI directory structure
mkdir -p /tmp/rustux-efi/EFI/BOOT
mkdir -p /tmp/rustux-efi/EFI/Rustux

# Copy kernel binaries
cp target/x86_64-unknown-uefi/release/rustux.efi /tmp/rustux-efi/EFI/BOOT/BOOTX64.EFI
cp target/x86_64-unknown-uefi/release/rustux.efi /tmp/rustux-efi/EFI/Rustux/kernel.efi

# Copy to disk image
mcopy -i rustux.img -s /tmp/rustux-efi/EFI ::

# Cleanup
rm -rf /tmp/rustux-efi

echo "✓ Bootable image created: rustux.img"
echo ""
echo "EFI files installed:"
echo "  /EFI/BOOT/BOOTX64.EFI (for direct boot)"
echo "  /EFI/Rustux/kernel.efi (for UEFI loader)"
