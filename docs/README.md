# Rustux Kernel Documentation

**Refactored UEFI x86_64 Kernel Architecture**

**Repository:** https://github.com/gitrustux/rustux

---

## Overview

The Rustux kernel is a refactored UEFI x86_64 kernel with cross-architecture interrupt controller support, ACPI-based hardware discovery, and Zircon-inspired capability-based security model.

**Last Updated:** 2026-01-18

---

## Repository Structure

### Main Repository (`https://github.com/gitrustux/rustux`)

**Working Directory:** `/var/www/rustux.com/prod/rustux/`

```
rustux/
├── Cargo.toml
└── src/
    ├── lib.rs              # Main library with panic handler
    ├── traits.rs           # Cross-architecture interrupt traits
    ├── arch/               # Architecture-specific code
    │   ├── amd64/          # x86_64 APIC implementation ✅
    │   ├── arm64/          # ARM64 GIC (stub)
    │   └── riscv64/        # RISC-V PLIC (stub)
    ├── interrupt/          # Generic interrupt handling
    ├── acpi/               # ACPI table parsing ✅
    ├── sched/              # Scheduler and thread management ✅
    ├── mm/                 # Memory management (migrated)
    ├── process/            # Process management (migrated)
    ├── sync/               # Synchronization primitives (migrated)
    ├── object/             # Kernel objects & handles (migrated)
    ├── syscall/            # System call handlers (migrated)
    └── drivers/            # Device drivers (migrated)
```

### Legacy Reference (Kept for Reference)

**Directory:** `/var/www/rustux.com/prod/kernel/` (NOT in git repo)

```
kernel/
├── kernel-efi/            # Legacy UEFI kernel (deprecated)
├── uefi-loader/           # UEFI bootloader
└── src/kernel/            # Original kernel source (migrated to rustux/)
```

### Documentation (In This Repository)

```
docs/
├── README.md                    # This file
├── REFACTORING_PLAN.md          # Refactoring progress & phases
├── TESTS.md                     # Test documentation
├── IMAGE.md                     # Disk image documentation
├── design/                      # Design documents
│   └── capability-system.md     # Zircon-inspired capability system design
└── decisions/                   # Architecture decisions
    └── structural-mistake.md    # Lessons learned
```

---

## Completed Work ✅

### Phase 1: Interrupt System Integration (Complete)

**Completed 2026-01-18:**
- ✅ IDT (Interrupt Descriptor Table) setup and handler installation
- ✅ GDT (Global Descriptor Table) configuration
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
- Architecture-agnostic API for x86_64 (APIC), ARM64 (GIC), RISC-V (PLIC)

**x86_64 APIC Implementation**
- Location: `src/arch/amd64/apic.rs`
- Local APIC initialization (UEFI firmware support)
- EOI handling, I/O APIC configuration
- `X86_64InterruptController` implements `InterruptController` trait

**ACPI MADT Parsing**
- Location: `src/acpi/`
- `rsdp.rs` - RSDP discovery from legacy BIOS locations
- `rsdt.rs` - RSDT/XSDT parsing with iterator
- `madt.rs` - MADT parsing for IOAPIC/LAPIC discovery
- Dynamic IOAPIC address discovery (not hardcoded)

**Scheduler/Thread Primitives**
- Location: `src/sched/`
- `state.rs` - ThreadState, ThreadPriority, RunQueue
- `thread.rs` - Thread, SavedRegisters, StackConfig
- `scheduler.rs` - Round-robin scheduler with priority

**Capability System Design**
- Location: `docs/design/capability-system.md`
- KernelObject trait
- Handle rights system (20+ right types)
- HandleTable per-process management
- Interrupt objects with port binding
- Zircon-inspired security model

---

## Migration Status

### Phase 2A: Keyboard Input Test Verification ✅ (Complete)
- QEMU test runs successfully with keyboard interrupts
- Debug log shows timer TICK messages
- Key presses generate [KEY:XX] scancode messages

### Phase 2B: Shell Migration with Interrupt-Driven Input ✅ (Complete)
- Shell migrated from `kernel/user/shell.rs` to `rustux/userspace/shell/`
- Polling input replaced with interrupt-driven keyboard handler
- Input buffer for keyboard events implemented

### Phase 2C: Full Kernel Component Migration ✅ (Complete)

#### Architecture Components (AMD64)
- ✅ `registers.rs` - CPU register access
- ✅ `tsc.rs` - Time Stamp Counter
- ✅ `syscall.rs` - Syscall entry point
- ✅ `uspace_entry.rs` - Userspace entry
- ✅ `ioport.rs` - Port I/O
- ✅ `bootstrap16.rs` - 16-bit bootstrap
- ✅ `cache.rs` - Cache operations
- ✅ `ops.rs` - Architecture operations
- ✅ `faults.rs` - Page fault handling
- ✅ `page_tables.rs` - Paging setup (in `mm/` subdirectory)
- ✅ `mmu.rs` - MMU operations

#### Memory Management
- ✅ `pmm.rs` - Physical memory manager
- ✅ `vm/*.rs` - Virtual memory manager (init, aspace, page_table, arch_vm_aspace)
- ✅ `allocator.rs` - Kernel allocator

#### Process & Thread Management
- ✅ `process/mod.rs` - Process management
- ✅ `thread/mod.rs` - Thread implementation (merged with existing scheduler)
- ✅ `init.rs` - Kernel initialization

#### Synchronization Primitives
- ✅ `sync/*.rs` - Synchronization module
- ✅ `mutex.rs` - Mutex implementation
- ✅ `spinlock.rs` - Spinlock implementation

#### Objects & Capabilities
- ✅ `object/mod.rs` - Object module
- ✅ `object/handle.rs` - Handle management
- ✅ `object/channel.rs` - Channel objects
- ✅ `object/event.rs` - Event objects
- ✅ `object/vmo.rs` - VMO (Virtual Memory Object)
- ✅ `object/timer.rs` - Timer objects
- ✅ `object/job.rs` - Job objects

#### System Calls
- ✅ `syscalls/mod.rs` - Syscall dispatch
- ✅ `syscalls/handle_ops.rs` - Handle operations
- ✅ `syscalls/object.rs` - Object syscalls
- ✅ `syscalls/vmo.rs` - VMO syscalls
- ✅ `syscalls/channel.rs` - Channel syscalls

#### Device Drivers
- ✅ `dev/uart/*.rs` - UART drivers (PL011, etc.)
- ✅ `dev/timer/*.rs` - Timer drivers
- ✅ `dev/interrupt/*.rs` - Interrupt controllers (GICv2, etc.)

#### ARM64 & RISC-V Support
- ✅ `arch/arm64/*.rs` - ARM64 interrupts, timer, threads
- ✅ `arch/riscv64/*.rs` - RISC-V PLIC
- ✅ `dev/interrupt/arm_gic/*` - ARM GIC support

---

## Critical Migration Rules ⚠️

### DO NOT MIGRATE (ACPI Conflicts)
These files were NOT migrated as they conflict with the new ACPI implementation in `rustux/src/acpi/`:

| Old Path | Reason | Replacement |
|----------|--------|-------------|
| `kernel/src/kernel/arch/amd64/include/arch/amd64/acpi.rs` | ACPI header | `rustux/src/acpi/` |
| `kernel/src/kernel/arch/amd64/acpi.S` | ACPI assembly | `rustux/src/acpi/` (Rust) |
| `kernel/src/kernel/platform/pc/acpi.cpp` | ACPI parsing | `rustux/src/acpi/` |
| `kernel/src/kernel/platform/pc/include/platform/pc/acpi.h` | ACPI header | `rustux/src/acpi/` |

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
- Proper EOI handling

### Boot Flow

```
UEFI Firmware
    ↓
uefi-loader (finds ACPI RSDP, builds memory map)
    ↓
KernelHandoff structure (passed to kernel)
    ↓
kernel init (IDT, GDT, paging setup, ACPI parsing)
    ↓
Interrupt controller initialization (APIC via discovered address)
    ↓
Scheduler starts
    ↓
Shell runs (interrupt-driven input)
```

---

## Testing

### QEMU Test Command

```bash
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
| `rustux/src/lib.rs` | Main library entry point |
| `rustux/src/traits.rs` | Interrupt controller trait |
| `rustux/src/acpi/*.rs` | ACPI parsing (RSDP, RSDT, MADT) |
| `rustux/src/arch/amd64/apic.rs` | x86_64 APIC implementation |
| `rustux/src/sched/*.rs` | Scheduler and thread management |
| `rustux/src/mm/*.rs` | Memory management (migrated) |
| `rustux/src/process/*.rs` | Process management (migrated) |
| `rustux/src/sync/*.rs` | Synchronization primitives (migrated) |
| `rustux/src/object/*.rs` | Kernel objects (migrated) |
| `rustux/src/syscall/*.rs` | System calls (migrated) |
| `rustux/src/drivers/*.rs` | Device drivers (migrated) |

### Documentation Files

| File | Purpose |
|------|---------|
| `docs/README.md` | This file |
| `docs/REFACTORING_PLAN.md` | Refactoring progress & phases |
| `docs/TESTS.md` | Test documentation |
| `docs/IMAGE.md` | Disk image documentation |
| `docs/design/capability-system.md` | Capability system design |
| `docs/decisions/structural-mistake.md` | Lessons learned |

---

## Build Commands

```bash
# Build the kernel
cargo build

# Build with release optimization
cargo build --release

# Run QEMU test
./test-qemu.sh
```

---

# Rustica Apps

Official application suite for the Rustux operating system.

## Overview

This repository contains all user-space applications for Rustux, organized into:
- **CLI utilities**: Essential command-line tools
- **GUI applications**: Aurora desktop environment components
- **Shared libraries**: Common utilities for app development

## Directory Structure

See [STRUCTURE.md](STRUCTURE.md) for the complete directory layout.

## Building

```bash
# Build all apps
./scripts/build-all.sh

# Build specific app
cargo build -p redit

# Build with release optimization
cargo build --release -p redit
```

## Repository Structure

- `cli/` - Command-line utilities
- `gui/` - Desktop applications (Aurora)
- `libs/` - Shared Rust libraries
- `examples/` - Example applications
- `tests/` - Integration tests
- `scripts/` - Build and deployment scripts

---

## License

MIT - See LICENSE file for details.
