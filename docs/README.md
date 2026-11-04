# Rustux Kernel Documentation

**UEFI x86_64 microkernel written in Rust**

**Repository:** https://github.com/gitrustux/rustux

---

## Overview

Rustux is a UEFI x86_64 kernel with a cross-architecture interrupt controller trait, ACPI-based hardware discovery, and a capability-based security model inspired by Zircon.

The kernel builds as a single UEFI application (`rustux.efi`). The application is installed as `EFI/BOOT/BOOTX64.EFI`, finds the ACPI RSDP, calls `ExitBootServices`, and then runs the kernel. No separate bootloader is used.

**Last Updated:** 2026-10-31

---

## Repository Structure

The repository root holds the kernel crate (`Cargo.toml` defines the `rustux` library and the `rustux` UEFI binary).

```
./
├── Cargo.toml               # rustux lib + rustux bin (feature: uefi_kernel)
├── .cargo/config.toml       # default target x86_64-unknown-uefi, 128 KB stack
├── build.rs
├── build.sh                 # Build kernel + create bootable image
├── build                    # Older copy of build.sh
├── build-live-image.sh      # Build the Rustica live USB image
├── test-qemu.sh             # QEMU interrupt test (checks for [TICK])
├── quick-test.sh            # QEMU memory-mapping debug run ([MAP] output)
├── scripts/
│   └── create-bootable-image.sh
├── docs/                    # This documentation
├── files/test.txt           # Ramdisk test file
├── test-hello/              # Minimal asm userspace binary
├── test-userspace/          # Early C test programs (hello)
├── userspace/
│   ├── build.sh             # Builds the C library and programs into bin/
│   ├── libc/                # The C library (docs/USERSPACE.md)
│   ├── c-progs/             # Programs: init, shell, commands, tests
│   ├── bin/                 # Built programs, embedded in the ramdisk
│   └── test/                # Rust userspace test binary (x86_64-unknown-none)
└── src/
    ├── lib.rs               # Library root and module declarations
    ├── main.rs              # UEFI entry point (#[entry]); requires uefi_kernel
    ├── init.rs              # Kernel initialization
    ├── traits.rs            # Cross-architecture InterruptController trait
    ├── test_entry.rs        # Kernel test entry (feature: kernel_test)
    ├── acpi/                # RSDP, RSDT/XSDT, MADT parsing
    ├── arch/
    │   ├── amd64/           # x86_64: APIC, GDT/IDT, MMU, syscall, userspace entry
    │   │   └── mm/          # Page tables, constants
    │   ├── arm64/           # GIC interrupt controller, MMU
    │   │   ├── interrupt/gic.rs
    │   │   └── mm/
    │   └── riscv64/         # PLIC interrupt controller, MMU
    │       ├── interrupt/plic.rs
    │       └── mm/
    ├── interrupt/           # Generic interrupt handling
    ├── mm/                  # PMM, kernel heap allocator
    ├── sched/               # Scheduler, threads, run queue
    ├── process/             # Processes, address spaces, context switch
    ├── sync/                # Spinlock, event, wait queue
    ├── object/              # Kernel objects and handles
    ├── syscall/             # Syscall dispatch, file descriptors
    ├── exec/                # ELF loader, process loader
    ├── fs/                  # VFS and ramdisk
    ├── drivers/             # UART, PS/2 keyboard, framebuffer display
    ├── testing/             # Test harness, QEMU exit helpers
    └── tests/               # Syscall tests
```

### Related Repositories

These repositories are separate from this one:

| Repository | Contents |
|------------|----------|
| [gitrustux/loader](https://github.com/gitrustux/loader) | Legacy UEFI kernel (`kernel-efi/`, including USB/xHCI work), `uefi-loader/`, `rustux_macros/` |
| [gitrustux/apps](https://github.com/gitrustux/apps) | Rustica userspace applications (CLI tools, Aurora GUI, shared libs) |
| [gitrustux/rustica](https://github.com/gitrustux/rustica) | Rustica OS distribution: images, releases, package repo, update system. Includes this kernel as a git submodule at `repo/kernel`. Disk image docs are in `rustica/docs/IMAGE.md` |

### Documentation (In This Repository)

```
docs/
├── README.md                     # This file
├── ARCHITECTURE.md               # Kernel architecture
├── OBJECTS.md                    # Kernel object reference
├── SYSCALL.md                    # System call reference
├── TESTS.md                      # Build, QEMU testing and debugging
├── ROADMAP.md                    # Architecture checklist (Jan 2026 plan)
├── HISTORY.md                    # Project history and decisions
├── usb-keyboard.md               # USB keyboard status and debugging history
└── design/
    ├── capability-system.md      # Capability system design
    ├── scheduler-and-ipc.md      # Scheduler and IPC design target
    ├── syscall-abi-v1.md         # Planned syscall ABI (draft)
    ├── hal-traits.md             # Planned hardware abstraction traits
    └── planning-notes.md         # Original design notes (reference)
```

---

## Completed Work ✅

### Phase 1: Interrupt System Integration (Complete)

**Completed 2026-01-18:**
- ✅ IDT setup and handler installation (`arch/amd64/idt.rs`)
- ✅ GDT configuration (`arch/amd64/descriptor.rs`)
- ✅ Timer interrupt handler (vector 32) with periodic TICK output
- ✅ Keyboard interrupt handler (vector 33) with scancode capture
- ✅ APIC initialization and IRQ routing
- ✅ QEMU test infrastructure with debug logging

**Test Results:**
```
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
```

### Core Infrastructure (Complete)

**Cross-Architecture Interrupt Controller Trait**
- Location: `src/traits.rs`
- Implemented by `X86_64InterruptController` (APIC), `Arm64InterruptController` (GIC) and `RiscvInterruptController` (PLIC), re-exported from `src/arch/mod.rs`

**x86_64 APIC Implementation**
- Location: `src/arch/amd64/apic.rs`, `src/arch/amd64/controller.rs`
- Local APIC initialization, EOI handling, I/O APIC configuration

**ACPI MADT Parsing**
- Location: `src/acpi/`
- `rsdp.rs` - RSDP discovery
- `rsdt.rs` - RSDT/XSDT parsing with iterator
- `madt.rs` - MADT parsing for IOAPIC/LAPIC discovery
- Dynamic IOAPIC address discovery (not hardcoded)

**Scheduler/Thread Primitives**
- Location: `src/sched/`
- `state.rs` - ThreadState, ThreadPriority, RunQueue
- `thread.rs` - Thread, SavedRegisters, StackConfig
- `scheduler.rs` - Scheduler with priority
- `round_robin.rs` - Round-robin scheduling

**Capability System Design**
- Location: `docs/design/capability-system.md`
- KernelObject trait, handle rights, per-process HandleTable
- Interrupt objects with port binding (design only)

---

## Migration Status

### Phase 2A: Keyboard Input Test Verification ✅ (Complete)
- QEMU test runs successfully with keyboard interrupts
- Debug log shows timer TICK messages
- Key presses generate [KEY:XX] scancode messages

### Phase 2B: Shell with Interrupt-Driven Input ✅ (Complete)
- Shell source: `userspace/c-progs/shell.c`
- PS/2 keyboard driver: `src/drivers/keyboard/` (`ps2.rs`, `layout.rs`)

### Phase 2C: Kernel Component Migration ✅ (Complete)

#### Architecture Components (`src/arch/amd64/`)
- ✅ `registers.rs` - CPU register access
- ✅ `tsc.rs` - Time Stamp Counter
- ✅ `descriptor.rs` - GDT/TSS
- ✅ `idt.rs` - Interrupt Descriptor Table
- ✅ `syscall.rs` - Syscall entry point
- ✅ `uspace.rs`, `uspace_entry.rs` - Userspace entry
- ✅ `ioport.rs` - Port I/O
- ✅ `bootstrap16.rs` - 16-bit bootstrap
- ✅ `cache.rs` - Cache operations
- ✅ `ops.rs` - Architecture operations
- ✅ `faults.rs` - Page fault handling
- ✅ `mmu.rs` - MMU operations
- ✅ `mm/page_tables.rs` - Paging setup
- ✅ `mexec.rs` - Userspace execution
- ✅ `switch.S` - Context switch
- ✅ `init.rs` - Architecture initialization

#### Memory Management (`src/mm/`)
- ✅ `pmm.rs` - Physical memory manager
- ✅ `allocator.rs` - Kernel heap allocator (global allocator after `ExitBootServices`)
- Virtual memory: page tables in `arch/*/mm/`, address spaces in `process/address_space.rs`

#### Process & Thread Management
- ✅ `process/mod.rs` - Process management
- ✅ `process/table.rs` - Process table
- ✅ `process/address_space.rs` - Per-process address spaces
- ✅ `process/switch.rs` - Process switching
- ✅ `sched/thread.rs` - Thread implementation
- ✅ `init.rs` - Kernel initialization

#### Synchronization Primitives (`src/sync/`)
- ✅ `spinlock.rs` - SpinMutex / SpinLock
- ✅ `event.rs` - Event
- ✅ `wait_queue.rs` - Wait queue

#### Objects & Capabilities (`src/object/`)
- ✅ `handle.rs` - Handle management
- ✅ `channel.rs` - Channel objects
- ✅ `event.rs` - Event objects
- ✅ `vmo.rs` - VMO (Virtual Memory Object)
- ✅ `timer.rs` - Timer objects
- ✅ `job.rs` - Job objects

#### System Calls (`src/syscall/`)
- ✅ `mod.rs` - Syscall dispatch and handlers
- ✅ `fd.rs` - File descriptor table

#### Executables & Filesystem
- ✅ `exec/elf.rs` - ELF loader
- ✅ `exec/process_loader.rs` - Process loader
- ✅ `fs/vfs.rs` - Virtual filesystem
- ✅ `fs/ramdisk.rs` - Embedded ramdisk

#### Device Drivers (`src/drivers/`)
- ✅ `uart.rs` - 16550 UART (COM1–COM4)
- ✅ `keyboard/` - PS/2 keyboard driver and layout
- ✅ `display/` - Framebuffer, font, text console

#### ARM64 & RISC-V Support
- ✅ `arch/arm64/` - GIC (`interrupt/gic.rs`), MMU (`mm/`)
- ✅ `arch/riscv64/` - PLIC (`interrupt/plic.rs`), MMU (`mm/`)
- The build is configured for `x86_64-unknown-uefi` only.

### Phase 7A: APIC and Keyboard Fixes (Merged from `loader`)

These changes were made in the `loader` repository's copy of the kernel and were merged here on 2026-10-31:
- Local APIC EOI written at offset `0xB0` (was `0x40`)
- `IA32_APIC_BASE` MSR handling and x2APIC detection
- PS/2 controller presence check; the kernel continues without a keyboard if none is found
- `switch.S` is not assembled for UEFI targets (context switching unavailable)
- The kernel global allocator is enabled by the `standalone` feature, which `uefi_kernel` turns on

---

## Known Issues

**Keyboard on real hardware.** The kernel has only a PS/2 (8042) keyboard driver. On USB-only machines no 8042 controller responds, so no keyboard input arrives. USB HID support (xHCI) is the planned fix. Experimental USB/xHCI code is in the `loader` repository (`kernel-efi/src/usb/`, latest work on branch `wip/xhci-2026-08`). See `docs/usb-keyboard.md` for the full history.

**Local APIC register struct.** In `src/arch/amd64/apic.rs`, the `LocalApic` struct offsets do not match the Intel SDM layout (registers sit on 16-byte boundaries). The kernel does not use the struct: EOI (`0xB0`) and TPR (`0x80`) are written at the correct offsets.

---

## Critical Migration Rules ⚠️

### DO NOT MIGRATE (ACPI Conflicts)
These legacy files conflict with the Rust ACPI implementation in `src/acpi/` and must not be migrated:

| Old Path | Reason | Replacement |
|----------|--------|-------------|
| `kernel/src/kernel/arch/amd64/include/arch/amd64/acpi.rs` | ACPI header | `src/acpi/` |
| `kernel/src/kernel/arch/amd64/acpi.S` | ACPI assembly | `src/acpi/` (Rust) |
| `kernel/src/kernel/platform/pc/acpi.cpp` | ACPI parsing | `src/acpi/` |
| `kernel/src/kernel/platform/pc/include/platform/pc/acpi.h` | ACPI header | `src/acpi/` |

---

## Architecture Overview

### Interrupt System

```
┌────────────────────────────────────────────────────────────────────────┐
│                         HARDWARE                                       │
│  ┌─────────────┐    ┌──────────────┐    ┌──────────────┐    ┌─────┐  │
│  │   Device    │ →  │   IOAPIC     │ →  │  Local APIC  │ →  │ CPU │  │
│  │ (Keyboard)  │    │ (Discovered  │    │   (0xFEE0...) │    │     │  │
│  │             │    │  via ACPI)   │    │              │    │     │  │
│  └─────────────┘    └──────────────┘    └──────────────┘    └─────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

**Key Features:**
- Dynamic IOAPIC discovery via ACPI MADT parsing
- Cross-architecture interrupt controller trait
- Vector-based IRQ routing (IRQ1 → Vector 33, etc.)
- EOI handling

### Boot Flow

```
UEFI Firmware
    ↓
EFI/BOOT/BOOTX64.EFI  (rustux.efi, entry point in src/main.rs)
    ↓
Find ACPI RSDP, ExitBootServices
    ↓
Kernel init (GDT, IDT, paging, heap, ACPI parsing)
    ↓
Interrupt controller initialization (APIC via discovered address)
    ↓
Scheduler starts
    ↓
Init (PID 1) → Shell (PID 2)
```

---

## Testing

### QEMU Test

```bash
./test-qemu.sh
```

This builds `rustux.img` with `scripts/create-bootable-image.sh` and runs:

```bash
qemu-system-x86_64 \
  -bios /usr/share/ovmf/OVMF.fd \
  -drive file=rustux.img,format=raw \
  -nographic \
  -device isa-debugcon,iobase=0xE9,chardev=debug \
  -chardev file,id=debug,path=/tmp/rustux-qemu-debug.log \
  -m 512M \
  -machine q35 \
  -smp 1 \
  -no-reboot \
  -no-shutdown
```

The script passes if `/tmp/rustux-qemu-debug.log` contains `[TICK]` messages.

### Expected Debug Output

```bash
cat /tmp/rustux-qemu-debug.log
```

Should show:
- ACPI RSDP discovery
- IDT/GDT configuration
- Interrupt handler installation
- Timer TICK messages
- Keyboard scancode events on keypress

---

## Key Files Reference

### Core Kernel Files

| File | Purpose |
|------|---------|
| `src/main.rs` | UEFI entry point |
| `src/lib.rs` | Library root |
| `src/init.rs` | Kernel initialization |
| `src/traits.rs` | Interrupt controller trait |
| `src/acpi/*.rs` | ACPI parsing (RSDP, RSDT, MADT) |
| `src/arch/amd64/apic.rs` | x86_64 APIC implementation |
| `src/sched/*.rs` | Scheduler and threads |
| `src/mm/*.rs` | Physical memory and heap |
| `src/process/*.rs` | Processes and address spaces |
| `src/sync/*.rs` | Synchronization primitives |
| `src/object/*.rs` | Kernel objects and handles |
| `src/syscall/*.rs` | System calls |
| `src/exec/*.rs` | ELF and process loading |
| `src/fs/*.rs` | VFS and ramdisk |
| `src/drivers/` | UART, keyboard, display |

### Documentation Files

| File | Purpose |
|------|---------|
| `docs/README.md` | This file |
| `docs/ARCHITECTURE.md` | Kernel architecture |
| `docs/OBJECTS.md` | Kernel objects |
| `docs/SYSCALL.md` | System calls |
| `docs/TESTS.md` | Build, QEMU testing and debugging |
| `docs/ROADMAP.md` | Architecture checklist |
| `docs/HISTORY.md` | Project history and decisions |
| `docs/usb-keyboard.md` | USB keyboard status and debugging history |
| `docs/design/capability-system.md` | Capability system design |
| `docs/design/scheduler-and-ipc.md` | Scheduler and IPC design target |
| `docs/design/syscall-abi-v1.md` | Planned syscall ABI |
| `docs/design/hal-traits.md` | Planned hardware abstraction traits |
| `docs/design/planning-notes.md` | Original design notes |

---

## Build Commands

The UEFI binary needs the `uefi_kernel` feature, so a plain `cargo build` builds only the library.

```bash
# Build kernel and create bootable image (rustux.img)
./build.sh

# Build the kernel binary only
cargo build --release --bin rustux --features uefi_kernel --target x86_64-unknown-uefi
# Output: target/x86_64-unknown-uefi/release/rustux.efi

# Build the Rustica live USB image
./build-live-image.sh

# Run the QEMU interrupt test
./test-qemu.sh
```

---

## License

MIT - See LICENSE file for details.
