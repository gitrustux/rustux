#!/bin/bash
# Build script for Rustux Kernel
# Creates a bootable UEFI disk image with the kernel

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "===================================================================="
echo "              Rustux Kernel Build Script"
echo "===================================================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# ============================================================================
# Step 1: Check prerequisites
# ============================================================================

echo "[Step 1] Checking prerequisites..."

# Check for cargo
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}✗ Cargo not found${NC}"
    echo "  Install Rust from https://rustup.rs/"
    exit 1
fi
echo -e "  ${GREEN}✓${NC} Cargo found"

# Check for UEFI target
if ! rustup target list | grep -q "installed.*x86_64-unknown-uefi"; then
    echo "  Installing UEFI target..."
    rustup target add x86_64-unknown-uefi
fi
echo -e "  ${GREEN}✓${NC} UEFI target installed"

# Check for mkfs.fat
if ! command -v mkfs.fat &> /dev/null; then
    echo -e "${RED}✗ mkfs.fat not found${NC}"
    echo "  Install dosfstools:"
    echo "    Ubuntu/Debian: sudo apt install dosfstools"
    echo "    Fedora: sudo dnf install dosfstools"
    exit 1
fi
echo -e "  ${GREEN}✓${NC} mkfs.fat found"

# Check for mcopy
if ! command -v mcopy &> /dev/null; then
    echo -e "${RED}✗ mcopy not found${NC}"
    echo "  Install mtools:"
    echo "    Ubuntu/Debian: sudo apt install mtools"
    echo "    Fedora: sudo dnf install mtools"
    exit 1
fi
echo -e "  ${GREEN}✓${NC} mcopy found"

echo ""

# ============================================================================
# Step 2: Build the kernel
# ============================================================================

echo "[Step 2] Building kernel..."

# Build with UEFI features and userspace test
cargo build --release --bin rustux --features "uefi_kernel,userspace_test" --target x86_64-unknown-uefi

if [ $? -ne 0 ]; then
    echo -e "${RED}✗ Build failed${NC}"
    exit 1
fi

echo -e "${GREEN}✓${NC} Kernel built successfully"
echo ""

# ============================================================================
# Step 3: Create disk image
# ============================================================================

echo "[Step 3] Creating bootable disk image..."

# Remove old image if exists
if [ -f "rustux.img" ]; then
    rm rustux.img
fi

# Create 64MB disk image
dd if=/dev/zero of=rustux.img bs=1M count=64 2>&1 | grep -v "records\|bytes"

# Format as FAT32
mkfs.fat -F 32 rustux.img 2>&1 | grep -v "trying"

echo -e "${GREEN}✓${NC} Disk image created"
echo ""

# ============================================================================
# Step 4: Copy kernel to disk image
# ============================================================================

echo "[Step 4] Installing kernel to disk image..."

# Create EFI directory structure
mkdir -p /tmp/rustux-efi/EFI/BOOT
mkdir -p /tmp/rustux-efi/EFI/Rustux

# Copy kernel as both BOOTX64.EFI (for booting) and kernel.efi (for bootloader)
cp target/x86_64-unknown-uefi/release/rustux.efi /tmp/rustux-efi/EFI/BOOT/BOOTX64.EFI
cp target/x86_64-unknown-uefi/release/rustux.efi /tmp/rustux-efi/EFI/Rustux/kernel.efi

# Copy to disk image
mcopy -i rustux.img -s /tmp/rustux-efi/EFI ::

# Cleanup
rm -rf /tmp/rustux-efi

echo -e "${GREEN}✓${NC} Kernel installed to disk image"
echo ""

# ============================================================================
# Step 5: Summary
# ============================================================================

echo "===================================================================="
echo -e "${GREEN}Build Complete!${NC}"
echo "===================================================================="
echo ""
echo "Output files:"
echo "  - rustux.img              Bootable UEFI disk image"
echo "  - target/x86_64-unknown-uefi/release/rustux.efi  Kernel binary"
echo ""
echo "Disk image contents:"
mdir -i rustux.img ::/EFI
echo ""
echo -e "${YELLOW}To test in QEMU, run:${NC}"
echo "  ./test-qemu.sh"
echo ""
echo "Or manually:"
echo "  qemu-system-x86_64 \\"
echo "    -bios /usr/share/ovmf/OVMF.fd \\"
echo "    -drive file=rustux.img,format=raw \\"
echo "    -serial stdio \\"
echo "    -m 512M"
echo ""
