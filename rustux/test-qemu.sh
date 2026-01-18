#!/bin/bash
# Test script for Rustux Kernel with Interrupt System Test
# Runs the kernel in QEMU and captures debug console output

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

echo "===================================================================="
echo "              Rustux Kernel - Interrupt System Test"
echo "===================================================================="
echo ""

# Check for QEMU
if ! command -v qemu-system-x86_64 &> /dev/null; then
    echo "QEMU not found! Please install QEMU:"
    echo "  Ubuntu/debian: sudo apt install qemu-system-x86 ovmf"
    exit 1
fi

# Find OVMF firmware
OVMF_PATHS=(
    "/usr/share/ovmf/OVMF.fd"
    "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd"
)

OVMF_FD=""
for path in "${OVMF_PATHS[@]}"; do
    if [ -f "$path" ]; then
        OVMF_FD="$path"
        break
    fi
done

if [ -z "$OVMF_FD" ]; then
    echo "ERROR: OVMF firmware not found!"
    echo "Install with: sudo apt install ovmf"
    exit 1
fi

echo "Prerequisites OK"
echo ""

# First create the bootable image
echo "Creating bootable image..."
./scripts/create-bootable-image.sh

echo ""
echo "Starting QEMU..."
echo ""

# Run QEMU with debug console enabled
qemu-system-x86_64 \
    -bios "$OVMF_FD" \
    -drive file=rustux.img,format=raw \
    -nographic \
    -device isa-debugcon,iobase=0xE9,chardev=debug \
    -chardev file,id=debug,path=/tmp/rustux-qemu-debug.log \
    -m 512M \
    -machine q35 \
    -smp 1 \
    -no-reboot \
    -no-shutdown

echo ""
echo "===================================================================="
echo "Test complete - Checking for tick messages..."
echo ""

# Show debug console output
if [ -f "/tmp/rustux-qemu-debug.log" ]; then
    if grep -q "\[TICK\]" /tmp/rustux-qemu-debug.log; then
        echo "✅ SUCCESS - Timer ticks detected!"
        echo ""
        echo "Tick count:"
        grep -c "\[TICK\]" /tmp/rustux-qemu-debug.log
        echo ""
        echo "Last 20 lines of debug output:"
        tail -20 /tmp/rustux-qemu-debug.log
    else
        echo "❌ FAILED - No tick messages found"
        echo ""
        echo "Debug log contents:"
        cat /tmp/rustux-qemu-debug.log
    fi
else
    echo "❌ ERROR - No debug log created"
fi

echo ""
echo "Full debug log saved to: /tmp/rustux-qemu-debug.log"
