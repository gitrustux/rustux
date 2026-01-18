# Rustux Kernel Testing Procedures

## Overview

This document describes the complete testing setup for the Rustux UEFI kernel, including build procedures, disk image creation, and QEMU testing.

## Directory Structure

```
/var/www/rustux.com/
├── prod/rustux/               # Main kernel source (refactored)
│   ├── src/
│   │   ├── main.rs            # Kernel entry point
│   │   ├── init.rs            # Kernel initialization
│   │   ├── lib.rs             # Main library with panic handler
│   │   ├── traits.rs          # Cross-architecture interrupt traits
│   │   ├── arch/              # Architecture-specific code
│   │   │   ├── amd64/         # x86_64 APIC, IDT, GDT, etc.
│   │   │   ├── arm64/         # ARM64 GIC (stub)
│   │   │   └── riscv64/       # RISC-V PLIC (stub)
│   │   ├── acpi/              # ACPI table parsing
│   │   ├── sched/             # Scheduler and thread management
│   │   ├── interrupt/         # Generic interrupt handling
│   │   └── testing/           # Test harness and QEMU configuration
│   ├── Cargo.toml             # Rust project config
│   ├── build.sh               # Build script
│   └── test-qemu.sh           # QEMU test script
├── html/rustica/
│   └── rustica-live-amd64-0.1.0.img  # Bootable disk image (GPT + FAT32 ESP)
└── prod/docs/
    └── TESTS.md               # This file
```

## Build Procedure

### 1. Build the Kernel

```bash
cd /var/www/rustux.com/prod/rustux
./build.sh
```

Or manually:
```bash
cd /var/www/rustux.com/prod/rustux
cargo build --release
```

**Output**: Built kernel binary in `target/x86_64-unknown-uefi/release/`

### 2. Create Bootable Disk Image

The build script automatically creates a bootable UEFI disk image:

```bash
./build.sh
```

This creates `rustux.img` - a 512MB GPT disk with FAT32 EFI System Partition containing the kernel.

### 3. Create/Update Disk Image (Manual)

If you need to manually update an existing disk image:

```bash
# Setup loop device
LOOPDEV=$(losetup -f)
losetup -P $LOOPDEV /var/www/rustux.com/prod/rustux/rustux.img
partprobe $LOOPDEV

# Mount EFI System Partition (partition 1)
mkdir -p /mnt/rustux-test
mount ${LOOPDEV}p1 /mnt/rustux-test

# Copy kernel (if needed)
cp target/x86_64-unknown-uefi/release/rustux.efi /mnt/rustux-test/EFI/BOOT/

# Unmount
umount /mnt/rustux-test
losetup -d $LOOPDEV
```

**Required paths on disk image**:
- `/EFI/BOOT/BOOTX64.EFI` - UEFI kernel/application

## QEMU Testing

### Basic Test (Interactive)

```bash
cd /var/www/rustux.com/prod/rustux
./test-qemu.sh
```

Or manually:

```bash
qemu-system-x86_64 \
  -bios /usr/share/ovmf/OVMF.fd \
  -drive file=rustux.img,format=raw \
  -m 512M \
  -machine q35
```

**Key options**:
- `-bios /usr/share/ovmf/OVMF.fd` - UEFI firmware (OVMF)
- `-drive file=rustux.img,format=raw` - Bootable disk image
- `-m 512M` - Memory size
- `-machine q35` - Q35 chipset

### Test with Debug Output

```bash
cd /var/www/rustux.com/prod/rustux
qemu-system-x86_64 \
  -bios /usr/share/ovmf/OVMF.fd \
  -drive file=rustux.img,format=raw \
  -debugcon file:/tmp/rustux-qemu-debug.log \
  -serial stdio \
  -m 512M \
  -machine q35 \
  -no-reboot \
  -no-shutdown
```

Check debug log after test:
```bash
cat /tmp/rustux-qemu-debug.log
```

## Boot Sequence

### 1. UEFI Firmware (OVMF) Loads
- Reads GPT partition table
- Finds EFI System Partition
- Loads `/EFI/BOOT/BOOTX64.EFI`

### 2. Kernel Entry
- UEFI firmware loads the kernel
- Kernel initialization begins
- IDT and GDT setup
- ACPI RSDP discovery
- Interrupt controller initialization
- Timer and keyboard handlers installed

### 3. Runtime Mode
- Kernel interrupts are active
- Timer ticks produce [TICK] messages
- Keyboard input produces [KEY:XX] scancode messages

## Current Kernel State (2026-01-18)

### Working ✅
- UEFI boot and kernel loading
- IDT/GDT setup
- Interrupt controller initialization (APIC via ACPI)
- Timer interrupt handler (produces [TICK] messages)
- Keyboard interrupt handler (produces [KEY:XX] scancodes)
- QEMU test infrastructure with debug logging

### Test Results

Expected debug output:
```
[OK] ACPI RSDP found: 0x...
[PHASE] Exiting boot services...
[1/5] Setting up GDT...
✓ GDT configured
[2/5] Setting up IDT...
✓ IDT configured
[3/5] Installing timer handler...
✓ Timer handler at vector 32
[3.5/5] Installing keyboard handler...
✓ Keyboard handler at vector 33
[4/5] Initializing APIC...
✓ APIC initialized
[4.5/5] Configuring keyboard IRQ...
✓ IRQ1 → Vector 33
[5/5] Configuring timer...
✓ Timer configured
[TICK]
[TICK]
```

Press keys in QEMU to see:
```
[KEY:1E]  ← Press 'A'
[KEY:9E]  ← Release 'A'
```

## Quick Test Commands

```bash
# Kill any existing QEMU
pkill -9 qemu

# Build
cd /var/www/rustux.com/prod/rustux
./build.sh

# Test with QEMU
./test-qemu.sh

# Check debug log
cat /tmp/rustux-qemu-debug.log
```

## References

- UEFI Specification
- OVMF (EDK2) for x86_64 UEFI firmware
- ACPI Specification (MADT table for interrupt controller discovery)
- GPT partition format
- FAT32 file system for ESP
- **Repository**: https://github.com/gitrustux/rustux
