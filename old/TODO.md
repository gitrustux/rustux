# Rustica OS - Boot Architecture Roadmap

## Overview

This document outlines the path to a native Rustux UEFI bootloader, distinguishing between **temporary installer requirements** and **long-term architectural goals**.

## Current State

- ✅ UEFI boot working with GRUB + Linux kernel (installer only)
- ✅ EFI/BOOT/BOOTX64.EFI present
- ✅ Custom initramfs that launches installer
- ⏳ Native Rustux kernel (not bootable via GRUB)
- ⏳ Native Rustux EFI loader (not implemented)

---

## Boot Models

### 🟢 Model A - Direct Boot (Final OS, Goal)

**Architecture:**
```
UEFI Firmware
    └── BOOTX64.EFI (Rustux EFI Loader)
         └── Rustux Kernel
              └── Rustux Userland
```

**Characteristics:**
- No Linux
- No GRUB
- Clean, independent architecture
- How Fuchsia, Windows, macOS, SerenityOS work

**Status:** ⏳ Future Goal

---

### 🟡 Model B - Linux-based Live Installer (Temporary)

**Architecture:**
```
UEFI Firmware
    └── GRUB
         └── Linux Kernel
              └── Initramfs
                   └── Rustux Installer
                        └── Writes Rustux EFI Loader to Disk
```

**Characteristics:**
- Linux is only a delivery vehicle
- Rustux OS does not depend on Linux at runtime
- Used only for installation media
- How Ubuntu, Fedora, Arch installers work

**Status:** ✅ Implemented (current)

---

## Implementation Plan

### Phase 1: Native UEFI Application (Pure EFI)

**Goal:** Create a minimal EFI executable that can run directly from firmware

**Status:** 🔄 In Progress

**Tasks:**
- [ ] Create `uefi-loader/` crate in kernel repository
- [ ] Add `uefi` target support (x86_64-unknown-uefi)
- [ ] Implement basic EFI entry point (`efi_main`)
- [ ] Handle EFI boot services
- [ ] Exit boot services and enter runtime
- [ ] Implement simple text output (EFI graphics protocol)
- [ ] Read filesystem from disk (EFI simple file protocol)

**Implementation Details:**
```rust
// uefi-loader/src/main.rs
#![no_std]
#![no_main]

use uefi_rs::*;

#[entry]
fn efi_main(_handle: Handle, _system_table: SystemTable<Boot>) -> Status {
    // 1. Initialize EFI console
    // 2. Display "Rustica OS" banner
    // 3. Load kernel from disk
    // 4. Jump to kernel entry point
}
```

**Dependencies:**
```toml
[dependencies]
uefi-rs = "0.30"  # EFI bindings
uefi-services = "0.27"  # EFI services
```

**Build Command:**
```bash
cargo build --target x86_64-unknown-uefi --release
```

---

### Phase 2: UEFI Bootloader

**Goal:** Create a proper bootloader that loads the Rustux kernel

**Tasks:**
- [ ] Implement PE/COFF header generation for EFI
- [ ] Create boot configuration (menu, timeout)
- [ ] Implement kernel loading from disk
- [ ] Pass memory map and boot information to kernel
- [ ] Support for both BIOS (legacy) and UEFI (optional)
- [ ] Add boot splash screen (optional)

**Bootloader Spec:**
```rust
// Boot configuration structure
struct BootConfig {
    kernel_path: String,      // e.g., "\\EFI\\Rustica\\kernel.bin"
    boot_args: Vec<String>,
    timeout: u32,
    default_entry: u32,
}

// Boot entry
struct BootEntry {
    name: String,
    kernel: String,
    initrd: Option<String>,   // For future: ramdisk
    args: Vec<String>,
}
```

**File Layout:**
```
EFI/
├── BOOT/
│   ├── BOOTX64.EFI    ← Main bootloader
│   └── rustica.efi    ← Rustux kernel (as EFI app)
└── Rustica/
    ├── kernel.bin
    └── boot.conf
```

---

### Phase 3: Kernel EFI Executable Format

**Goal:** Make Rustux kernel bootable as EFI application

**Tasks:**
- [ ] Modify kernel entry point for EFI environment
- [ ] EFI executable format (PE32+)
- [ ] Linker script for EFI relocation
- [ ] Handle EFI memory map
- [ ] Exit boot services before kernel takes over

**Linker Script:**
```ld
ENTRY(efi_main)

SECTIONS {
    .text : {
        *(.text)
        *(.text.*)
    }
    .data : {
        *(.data)
        *(.data.*)
    }
    .reloc : {
        *(.reloc)
    }
}
```

**Kernel Entry Point:**
```rust
// No more `kmain` - use EFI entry
#[no_mangle]
pub extern "C" fn efi_main(
    image_handle: *mut c_void,
    system_table: *mut EFISystemTable,
) -> usize {
    // 1. Initialize kernel
    // 2. Exit boot services
    // 3. Continue normal boot
}
```

---

### Phase 4: Clean Separation (Remove GRUB Dependency)

**Goal:** Eliminate Linux/GRUB from the final OS

**Tasks:**
- [ ] Keep Linux-based installer ONLY for installation media
- [ ] Installed systems use native Rustux bootloader
- [ ] Update build scripts for target separation
- [ ] Document dual-boot strategy (installer vs installed)

**Build Targets:**
```bash
# Build for installation (creates bootable USB)
make installer-iso
├── Uses Linux kernel temporarily
├── Includes installer tools
└── Writes native Rustux to target

# Build for deployment (final OS)
make rustux-efi
├── Native Rustux bootloader
├── Native Rustux kernel
└── No Linux anywhere
```

---

## Immediate Next Steps

### 1. Research UEFI Development

- [ ] Study EDK II source code
- [ ] Review UEFI Specification 2.10+
- [ ] Examine other Rust EFI projects:
  - `uefi-rs` crate examples
  - `rEFIt` bootloader
  - `Limine` bootloader
  - `OVMF` for testing

### 2. Set Up Development Environment

- [ ] Install QEMU with OVMF for EFI testing
  ```bash
  apt-get install qemu-system-x86 ovmf
  ```
- [ ] Create test EFI application
  ```bash
  cargo new uefi-test --bin
  # Add uefi-rs dependencies
  # Build with x86_64-unknown-uefi target
  ```
- [ ] Test in QEMU:
  ```bash
  qemu-system-x86_64 \
    -drive file=test.img,format=raw \
    -bios /usr/share/OVMF/OVMF.fd
  ```

### 3. Create Proof of Concept

- [ ] Minimal "Hello World" EFI application
- [ ] Display text on screen using EFI protocols
- [ ] Read files from disk
- [ ] Load and execute a simple payload

### 4. Design Native Bootloader

**Features:**
- [ ] Boot menu (multiple entries)
- [ ] Kernel selection
- [ ] Timeout with default boot
- [ ] Edit boot parameters
- [ ] Recovery mode
- [ ] Boot splash

**Configuration File Format:**
```toml
# /EFI/Rustica/boot.conf

[boot]
timeout = 5
default = 0

[[entries]]
name = "Rustica OS"
kernel = "\\EFI\\Rustica\\kernel.bin"
args = ["quiet", "splash"]

[[entries]]
name = "Rustica OS (Safe Mode)"
kernel = "\\EFI\\Rustica\\kernel.bin"
args = ["nomodeset", "verbose"]
```

---

## Architecture Decisions

### ✅ DO

- Use native Rust UEFI loader for final OS
- Keep Linux ONLY for installer (temporary)
- Use standard UEFI protocols
- Support multi-boot (USB, HDD, SSD)
- Implement clean boot process

### ❌ DON'T

- Don't make Rustux pretend to be Linux
- Don't permanently depend on GRUB
- Don't rewrite kernel for GRUB compatibility
- Don't introduce Linux dependencies into Rustux runtime
- Don't use legacy BIOS boot (unless required)

---

## Testing Strategy

### Unit Testing
- [ ] Test EFI loader with QEMU + OVMF
- [ ] Verify boot menu navigation
- [ ] Test kernel loading
- [ ] Memory map validation

### Integration Testing
- [ ] Boot from USB (real hardware)
- [ ] Boot from virtual machine
- [ ] Test installation process
- [ ] Verify installed system boots

### Hardware Testing
- [ ] Test on different UEFI firmware versions
- [ ] Test Secure Boot (later)
- [ ] Test various storage controllers
- [ ] Test different architectures (ARM64, RISC-V)

---

## Dependencies

### Required Crates

```toml
[dependencies]
uefi = "0.30"           # EFI bindings and types
uefi-services = "0.27"  # EFI services wrapper
uefi-exts = "0.5"       # Extended protocols

# For bootloader
log = "0.4"
bitflags = "2"

# For kernel (already have)
x86_64 = "0.14"
volatile = "0.4"
spin = "0.9"
```

### Build Tools

- `rustc` with `x86_64-unknown-uefi` target
- `cargo` with custom target specification
- `lld-link` or `gnu-efi` for linking
- QEMU + OVMF for testing

---

## Timeline Estimate

| Phase | Duration | Dependencies |
|-------|----------|--------------|
| Phase 1: EFI App | 2-3 weeks | UEFI research |
| Phase 2: Bootloader | 3-4 weeks | Phase 1 complete |
| Phase 3: Kernel EFI | 2-3 weeks | Phase 2 complete |
| Phase 4: Cleanup | 1 week | Phase 3 complete |

**Total:** ~8-11 weeks to fully native boot

---

## Resources

### Documentation
- [UEFI Specification](https://uefi.org/specifications)
- [UEFI-rs Documentation](https://docs.rs/uefi-rs/)
- [EDK II](https://github.com/tianocore/edk2)

### Reference Projects
- [Limine Bootloader](https://github.com/limine-bootloader/limine)
- [rEFInd](https://sourceforge.net/projects/refind/)
- [Fuchsia Zircon Boot](https://fuchsia.dev/fuchsia-src/development/booting/zircon)

### Testing Tools
- [QEMU](https://www.qemu.org/)
- [OVMF](https://github.com/tianocore/edk2)
- [DuetPkg](https://github.com/tianocore/tianocore/wiki/UEFI-DuetPkg)

---

## Notes

### Why This Architecture Matters

1. **Security**: No Linux attack surface in final OS
2. **Simplicity**: One bootloader, one kernel, clean chain
3. **Independence**: No external dependencies
4. **Performance**: Direct boot, no intermediate layers
5. **Control**: Full ownership of boot process

### Current Linux Installer is Fine

Using Linux for the installer is **not wrong** - it's a tool. The key is:
- Linux stays on the USB, not on installed system
- Installed system uses native Rustux bootloader
- Clear separation between installer and OS

---

*Last Updated: 2025-01-09*
*Status: Planning Phase - Phase 1 Ready to Start*
