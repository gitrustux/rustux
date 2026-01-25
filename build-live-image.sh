#!/usr/bin/env bash
#
# Rustux OS - Live USB Image Build Script
#
# Creates a bootable UEFI disk image for Rustux OS kernel.
# This is a UEFI application that boots directly without GRUB.
#
# Usage:
#   ./build-live-image.sh [version]
#
# Environment Variables:
#   RUSTUX_VERSION   Version string (default: 0.1.0)
#   OUTPUT_DIR       Output directory (default: /var/www/rustux.com/html/rustica)
#

set -e

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_SRC="${SCRIPT_DIR}/target/x86_64-unknown-uefi/release"
KERNEL_EFI="${KERNEL_SRC}/rustux.efi"
OUTPUT_DIR="${OUTPUT_DIR:-/var/www/rustux.com/html/rustica}"
BUILD_DIR="${SCRIPT_DIR}/.build-live"
VERSION="${RUSTUX_VERSION:-0.1.0}"
ARCH="${RUSTUX_ARCH:-amd64}"

# Image configuration
IMG_SIZE="128M"
IMG_NAME="rustica-live-${ARCH}-${VERSION}.img"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }

# Clean build directory
clean_build() {
    log_step "Cleaning build directory..."
    rm -rf "$BUILD_DIR"
    mkdir -p "$BUILD_DIR"
    mkdir -p "$OUTPUT_DIR"
}

# Check kernel is built
check_kernel() {
    log_step "Checking kernel build..."

    if [ ! -f "$KERNEL_EFI" ]; then
        log_error "Kernel not found at: $KERNEL_EFI"
        log_info "Build the kernel first:"
        log_info "  cd $SCRIPT_DIR"
        log_info "  cargo build --release --bin rustux --features uefi_kernel --target x86_64-unknown-uefi"
        exit 1
    fi

    local size=$(du -h "$KERNEL_EFI" | cut -f1)
    log_info "Kernel found: $KERNEL_EFI ($size)"
}

# Create disk image
create_disk_image() {
    log_step "Creating disk image (${IMG_SIZE})..."

    local img_path="$BUILD_DIR/$IMG_NAME"

    # Create sparse disk image
    dd if=/dev/zero of="$img_path" bs=1 count=0 seek="$IMG_SIZE" 2>/dev/null

    # Create GPT partition table with single EFI System Partition
    log_info "Creating GPT partition table..."
    parted "$img_path" mklabel gpt 2>/dev/null || true
    parted "$img_path" mkpart primary fat32 1MiB 100% 2>/dev/null || true
    parted "$img_path" set 1 esp on 2>/dev/null || true

    # Setup loop device
    local loop_dev
    loop_dev=$(losetup -f --show "$img_path")
    partprobe "$loop_dev" 2>/dev/null || true
    sleep 1

    # Format partition as FAT32
    log_info "Formatting partition as FAT32..."
    mkfs.vfat -F32 "${loop_dev}p1" 2>/dev/null

    # Mount partition
    local mount_dir="$BUILD_DIR/mount"
    mkdir -p "$mount_dir"
    mount "${loop_dev}p1" "$mount_dir" 2>/dev/null

    # Create EFI directory structure
    log_info "Creating EFI directory structure..."
    mkdir -p "$mount_dir/EFI/BOOT"

    # Copy kernel as BOOTX64.EFI
    log_info "Installing kernel as BOOTX64.EFI..."
    cp "$KERNEL_EFI" "$mount_dir/EFI/BOOT/BOOTX64.EFI"

    local kernel_size=$(du -h "$KERNEL_EFI" | cut -f1)
    log_info "Kernel installed: BOOTX64.EFI ($kernel_size)"

    # Unmount and cleanup
    log_info "Cleaning up..."
    sync
    umount "$mount_dir" 2>/dev/null || true
    losetup -d "$loop_dev" 2>/dev/null || true
    rm -rf "$mount_dir"

    # Copy to output
    cp "$img_path" "$OUTPUT_DIR/$IMG_NAME"
    ln -sf "$IMG_NAME" "$OUTPUT_DIR/rustica-live-${ARCH}.img"

    # Generate checksum
    cd "$OUTPUT_DIR"
    sha256sum "$IMG_NAME" > "${IMG_NAME}.sha256"

    local size=$(du -h "$OUTPUT_DIR/$IMG_NAME" | cut -f1)
    local checksum=$(cut -d' ' -f1 "${IMG_NAME}.sha256")

    log_info "Image created: $OUTPUT_DIR/$IMG_NAME ($size)"
    log_info "Symlink: $OUTPUT_DIR/rustica-live-${ARCH}.img"
    log_info "SHA256: $checksum"
}

# Main
main() {
    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║                                                           ║"
    echo "║              Rustux OS Live Image Build v${VERSION}            ║"
    echo "║                                                           ║"
    echo "║              Direct UEFI Boot (No GRUB)                   ║"
    echo "║                                                           ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    echo ""

    clean_build
    check_kernel
    create_disk_image

    echo ""
    echo "╔═══════════════════════════════════════════════════════════╗"
    echo "║                                                           ║"
    echo "║              Build completed successfully!               ║"
    echo "║                                                           ║"
    echo "║              Image is ready for UEFI boot testing         ║"
    echo "║                                                           ║"
    echo "║           Boot flow: UEFI → BOOTX64.EFI → shell           ║"
    echo "║                                                           ║"
    echo "╚═══════════════════════════════════════════════════════════╝"
    echo ""
}

main "$@"
