#!/bin/bash
# Build script for userspace test program

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_DIR="$(dirname "$SCRIPT_DIR")"

echo "Building userspace test program..."

cd "$SCRIPT_DIR"

# Build the userspace program
cargo build --release --target x86_64-unknown-none

# Get the ELF file
ELF_FILE="target/x86_64-unknown-none/release/rustux-userspace-test"

if [ ! -f "$ELF_FILE" ]; then
    echo "Error: Build failed - ELF file not found"
    exit 1
fi

# Extract raw binary (skip ELF headers for now, we'll use ELF loader later)
# For initial testing, we just need the raw code
objcopy -O binary "$ELF_FILE" "userspace-test.bin"

echo "Userspace test built successfully!"
echo "ELF: $ELF_FILE"
echo "Raw binary: $SCRIPT_DIR/userspace-test.bin"

# Show file info
ls -lh "$ELF_FILE" "userspace-test.bin"

# Show ELF sections for debugging
echo ""
echo "ELF sections:"
readelf -S "$ELF_FILE" || true

echo ""
echo "Entry point:"
readelf -h "$ELF_FILE" | grep Entry || true
