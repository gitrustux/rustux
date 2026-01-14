# Rustux Kernel Testing Procedures

## Overview

This document describes the complete testing setup for the Rustux UEFI kernel, including build procedures, disk image creation, and QEMU testing.

## Directory Structure

```
/var/www/rustux.com/
├── prod/kernel/kernel-efi/     # Main UEFI kernel source
│   ├── src/
│   │   ├── main.rs            # Kernel entry point, ExitBootServices handling
│   │   ├── runtime.rs         # Runtime initialization structures
│   │   ├── console.rs         # Console abstraction layer
│   │   ├── native_console.rs  # Native console (serial/framebuffer)
│   │   ├── filesystem.rs      # Embedded filesystem
│   │   └── theme.rs           # Color themes
│   ├── Cargo.toml             # Rust project config
│   └── target/x86_64-unknown-uefi/release/
│       └── rustux-kernel-efi.efi  # Compiled kernel binary
├── prod/kernel/uefi-loader/   # UEFI bootloader (separate project)
├── html/rustica/
│   └── rustica-live-amd64-0.1.0.img  # Bootable disk image (GPT + FAT32 ESP)
└── prod/tests.md              # This file
```

## Build Procedure

### 1. Build the Kernel

```bash
cd /var/www/rustux.com/prod/kernel/kernel-efi
cargo build --release --target x86_64-unknown-uefi
```

**Important**: Must use `--target x86_64-unknown-uefi` for UEFI target.
Output: `target/x86_64-unknown-uefi/release/rustux-kernel-efi.efi`

### 2. Build the Bootloader (if needed)

```bash
cd /var/www/rustux.com/prod/kernel/uefi-loader
cargo build --release --target x86_64-unknown-uefi
```

Output: `target/x86_64-unknown-uefi/release/rustux-uefi-loader.efi`

### 3. Create/Update Disk Image

The disk image is a 512MB GPT disk with FAT32 EFI System Partition:

```bash
# Setup loop device
LOOPDEV=$(losetup -f)
losetup -P $LOOPDEV /var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img
partprobe $LOOPDEV

# Mount EFI System Partition (partition 1)
mkdir -p /mnt/rustica-test
mount ${LOOPDEV}p1 /mnt/rustica-test

# Copy kernel
cp target/x86_64-unknown-uefi/release/rustux-kernel-efi.efi /mnt/rustica-test/EFI/Rustux/kernel.efi

# Unmount
umount /mnt/rustica-test
losetup -d $LOOPDEV
```

**Required paths on disk image**:
- `/EFI/BOOT/BOOTX64.EFI` - UEFI bootloader (loads kernel)
- `/EFI/Rustux/kernel.efi` - Rustux kernel

## QEMU Testing

### Basic Test (Interactive)

```bash
qemu-system-x86_64 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive format=raw,file=/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img \
  -nographic \
  -serial mon:stdio
```

**Key options**:
- `-drive if=pflash,...` - UEFI firmware (OVMF)
- `-nographic` - No GUI, serial/console only
- `-serial mon:stdio` - Redirect serial to stdin/stdout

### Test with expect (Automated)

```tcl
#!/usr/bin/expect
set timeout 30
spawn qemu-system-x86_64 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive format=raw,file=/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img \
  -nographic \
  -serial mon:stdio

expect "rustica>"
send "hello\r"

expect {
    "TRACE-D" { puts "\n=== ExitBootServices SUCCESS ===" }
    "FAILED" { puts "\n=== ExitBootServices FAILED ===" }
    timeout { puts "\n=== TIMEOUT ===" }
}
```

### Test with Timeout (Detect Hangs)

```bash
# If QEMU keeps running past timeout, ExitBootServices succeeded
timeout 10 qemu-system-x86_64 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive format=raw,file=/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img \
  -nographic \
  -serial mon:stdio
```

## Boot Sequence

### 1. UEFI Firmware (OVMF) Loads
- Reads GPT partition table
- Finds EFI System Partition
- Loads `/EFI/BOOT/BOOTX64.EFI`

### 2. Bootloader (rustux-uefi-loader) Runs
- Phase 1: UEFI Environment Initialization
- Phase 2: Platform Discovery (ACPI)
- Phase 3: Memory Map Acquisition
- Phase 4: Kernel Loading
  - Loads `/EFI/Rustux/kernel.efi`
  - Validates PE/COFF header
  - Calls `LoadImage` and `StartImage`

### 3. Kernel Entry (main.rs)
- TRACE-0: KERNEL ENTRY
- TRACE-1: CONTINUING
- Console initialization
- CLI mode starts
- Shows `rustica> ` prompt

### 4. ExitBootServices Transition (when `hello` command typed)

**Before ExitBootServices**:
- TRACE-A: About to disable interrupts
- TRACE-B: Interrupts disabled (CLI instruction)
- TRACE-C: About to call ExitBootServices
- TRACE-C1: Getting memory map size (first GetMemoryMap call)
- TRACE-C2: Allocating memory map buffer
- TRACE-C3: Getting actual memory map (second GetMemoryMap call)

**FROZEN ZONE** (CRITICAL):
- NO allocations
- NO console output
- NO protocol calls
- Only ExitBootServices call allowed

**After ExitBootServices**:
- TRACE-D: ExitBootServices RETURNED (may not appear - UEFI console dead)
- Kernel in runtime mode
- Must use custom allocator and native console

## Known Issues

### 1. Serial Port I/O Hang
**Symptom**: Writing to COM1 (0x3F8) causes QEMU to hang
**Workaround**: Avoid serial tracing for now
**Status**: Needs investigation

### 2. UEFI Console After ExitBootServices
**Symptom**: Console output doesn't appear after ExitBootServices
**Reason**: UEFI console protocols depend on boot services
**Solution**: Use native console driver (serial or framebuffer)

### 3. Memory Map Changed Error
**Symptom**: ExitBootServices fails with "memory map changed"
**Root Cause**: Any allocation/print between GetMemoryMap and ExitBootServices
**Solution**: Strict frozen zone implementation

## Current Kernel State (2025-01-13)

### Working ✅
- UEFI boot and kernel loading
- UEFI console output (before ExitBootServices)
- **ExitBootServices with proper frozen zone** (FIXED!)
- **Transition to runtime mode** (confirmed via QEMU timeout test)
- CPU alive in runtime mode (probe loop confirms execution continues)

### Fixed Issues
- **Memory map changed error**: Fixed by implementing strict frozen zone between GetMemoryMap and ExitBootServices
- **GetMemoryMap hang**: Was caused by debug output after GetMemoryMap changing memory map

### In Progress
- Post-ExitBootServices initialization
- Custom memory allocator (UEFI allocator doesn't work after ExitBootServices)
- Native console driver (UEFI console dead, serial has issues)

### TODO
- Fix serial port I/O hangs (COM1 @ 0x3F8 causes QEMU hang)
- Implement proper runtime memory management
- External program execution
- Exception handlers and interrupt controller
- Scheduler

## Trace Points

Used for debugging ExitBootServices:

| Trace | Location | Purpose |
|-------|----------|---------|
| TRACE-0 | Kernel entry | Confirm kernel loaded |
| TRACE-1 | After entry | Continue execution |
| TRACE-A | Before CLI | About to disable interrupts |
| TRACE-B | After CLI | Interrupts disabled |
| TRACE-C | Before ExitBootServices | About to start process |
| TRACE-C1 | First GetMemoryMap | Get buffer size |
| TRACE-C2 | Allocate buffer | Allocate memory map buffer |
| TRACE-C3 | Second GetMemoryMap | Get actual memory map |
| TRACE-D | After ExitBootServices | Confirm return (may not appear) |

## Debugging ExitBootServices

### If hung at TRACE-C3

**Problem**: GetMemoryMap second call hanging or failing

**Check**:
1. Is buffer allocated correctly?
2. Is buffer size sufficient?
3. Is memory map changing between calls?

### If hung between C3 and D

**Problem**: ExitBootServices hanging inside call

**Check**:
1. Frozen zone violation (console output between GetMemoryMap and ExitBootServices)
2. Memory map key invalid
3. Interrupts firing during call

### If no output after D

**Problem**: Expected - UEFI console is dead

**Check**:
1. QEMU still running? (Yes = ExitBootServices succeeded)
2. CPU in probe loop? (Yes = kernel alive)

## Quick Test Commands

```bash
# Kill any existing QEMU
pkill -9 qemu

# Build
cd /var/www/rustux.com/prod/kernel/kernel-efi
cargo build --release --target x86_64-unknown-uefi

# Update image
LOOPDEV=$(losetup -f)
losetup -P $LOOPDEV /var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img
partprobe $LOOPDEV
mount ${LOOPDEV}p1 /mnt/rustica-test
cp target/x86_64-unknown-uefi/release/rustux-kernel-efi.efi /mnt/rustica-test/EFI/Rustux/kernel.efi
umount /mnt/rustica-test
losetup -d $LOOPDEV

# Test
timeout 10 qemu-system-x86_64 \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
  -drive format=raw,file=/var/www/rustux.com/html/rustica/rustica-live-amd64-0.1.0.img \
  -nographic -serial mon:stdio
```

## References

- UEFI Specification: ExitBootServices() requirements
- OVMF (EDK2) for x86_64 UEFI firmware
- rust-uefi crate documentation
- GPT partition format
- FAT32 file system for ESP
