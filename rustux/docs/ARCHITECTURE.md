# Rustux Kernel - Architecture Documentation

**Version:** 0.2.0
**Date:** 2025-01-18
**Status:** Phase 3A - Kernel booted, interrupt system working

---

## Table of Contents

1. [Overview](#overview)
2. [Design Philosophy](#design-philosophy)
3. [Architecture Diagram](#architecture-diagram)
4. [Module Organization](#module-organization)
5. [Boot Sequence](#boot-sequence)
6. [Interrupt System](#interrupt-system)
7. [Memory Management](#memory-management)
8. [Process Management](#process-management)
9. [System Calls](#system-calls)
10. [Kernel Objects](#kernel-objects)
11. [Device Drivers](#device-drivers)

---

## Overview

The Rustux kernel is a **microkernel** inspired by Zircon (Fuchsia OS). It follows a capability-based security model and prioritizes minimalism, safety, and modularity.

### Key Characteristics

- **Language:** Rust (no unsafe code where avoidable)
- **Architecture:** x86_64 (AMD64) - fully implemented
- **Boot Method:** UEFI
- **Kernel Type:** Microkernel
- **Security Model:** Capability-based (Zircon-style handles)
- **License:** MIT

### Design Goals

1. **Safety First:** Leverage Rust's type system for memory safety
2. **Minimal Trusted Base:** Keep the kernel small and auditable
3. **Capability Security:** Use handles for all privileged operations
4. **Modular Architecture:** Clear separation between kernel components
5. **Portability:** Architecture-agnostic core where possible

---

## Design Philosophy

### Microkernel vs Monolithic

Rustux follows the **microkernel** approach:

| Aspect | Microkernel (Rustux) | Monolithic (Linux) |
|--------|---------------------|-------------------|
| Kernel Size | Small (~200KB) | Large (MBs) |
| Drivers | Userspace | Kernelspace |
| Failure Isolation | High | Low |
| IPC Overhead | Higher | Lower |
| Security | Better | Worse |

### Zircon Influence

- **Handle-based:** All kernel resources accessed via handles
- **Rights:** Handles have associated rights (READ, WRITE, EXECUTE)
- **Objects:** Everything is a kernel object (VMO, Process, Thread, etc.)
- **No Root:** Capability-based instead of UID/GID

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         Userspace                               │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
│  │  sh      │  │   init   │  │   pkg    │  │  fwctl   │       │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘       │
│       └─────────────┴─────────────┴─────────────┴─────┐         │
│                      │ System Call Interface          │         │
└──────────────────────┼────────────────────────────────┘         │
                       │                                         │
┌──────────────────────┼─────────────────────────────────────────┐
│                      ▼                                         │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              System Call Handler (syscall/)              │  │
│  │  - Validates arguments                                    │  │
│  │  - Checks handle rights                                   │  │
│  │  - Dispatches to object managers                          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                              │  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                  Kernel Objects (object/)                 │  │
│  │  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐  │  │
│  │  │ VMO  │ │Process│ │Thread│ │Event │ │Timer │ │Channel│  │  │
│  │  └───┬──┘ └───┬──┘ └───┬──┘ └───┬──┘ └───┬──┘ └───┬──┘  │  │
│  │      └─────────┴─────────┴─────────┴─────────┴─────────┘  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │               Process Manager (process/)                  │  │
│  │  - Process creation/destruction                           │  │
│  │  - Thread management                                      │  │
│  │  - Address space management                               │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                Scheduler (sched/)                         │  │
│  │  - Thread scheduling                                      │  │
│  │  - CPU time allocation                                    │  │
│  │  - Priority management                                    │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Memory Manager (mm/)                         │  │
│  │  - Physical Memory Manager (PMM)                          │  │
│  │  - Page Allocator                                        │  │
│  │  - Address space mapping                                  │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │           Architecture Layer (arch/)                      │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐         │  │
│  │  │   amd64    │  │   arm64    │  │  riscv64   │         │  │
│  │  │ (COMPLETE) │  │ (PLACEHOLDER)│ │ (PLACEHOLDER)│       │  │
│  │  └─────┬──────┘  └─────┬──────┘  └─────┬──────┘         │  │
│  │        └──────────────────┴──────────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Synchronization (sync/)                      │  │
│  │  - SpinLock, Mutex                                        │  │
│  │  - WaitQueue                                             │  │
│  │  - SyncEvent                                             │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │               Interrupt System (interrupt/)               │  │
│  │  - 8259 PIC (legacy)                                      │  │
│  │  - APIC (modern)                                          │  │
│  │  - IRQ routing                                            │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                Device Drivers (drivers/)                  │  │
│  │  - UART (serial console)                                  │  │
│  │  - Keyboard (IRQ1)                                        │  │
│  │  - Timer (IRQ0)                                           │  │
│  └──────────────────────────────────────────────────────────┘  │
│                              │                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Hardware                               │  │
│  │  CPU, Memory, APIC, PIC, UART, Keyboard, Timer           │  │
│  └──────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Module Organization

### Core Modules

| Module | Purpose | Status |
|--------|---------|--------|
| `main.rs` | Kernel entry point | ✅ Complete |
| `lib.rs` | Module declarations | ✅ Complete |
| `init.rs` | Boot initialization | ✅ Complete |
| `test_entry.rs` | Test entry point | ✅ Complete |
| `traits.rs` | Common traits | ✅ Complete |

### Architecture Modules (`arch/`)

| Module | Description | Status |
|--------|-------------|--------|
| `amd64/` | x86_64 architecture (fully implemented) | ✅ Complete |
| `arm64/` | ARM64 architecture (placeholder) | 🔶 Placeholder |
| `riscv64/` | RISC-V architecture (placeholder) | 🔶 Placeholder |

### AMD64 Submodules

| File | Purpose | Lines |
|------|---------|-------|
| `bootstrap16.rs` | 16-bit boot code | ~150 |
| `cache.rs` | Cache management | ~100 |
| `descriptor.rs` | GDT/IDT descriptors | ~300 |
| `faults.rs` | Exception handlers | ~250 |
| `idt.rs` | Interrupt Descriptor Table | ~200 |
| `init.rs` | AMD64 initialization | ~400 |
| `ioport.rs` | Port I/O | ~100 |
| `mm/` | Memory management | ~500 |
| `ops.rs` | CPU operations | ~200 |
| `registers.rs` | CPU registers | ~150 |
| `syscall.rs` | System call interface | ~200 |
| `tsc.rs` | Time Stamp Counter | ~100 |
| `uspace_entry.rs` | Userspace entry | ~150 |

### Memory Management (`mm/`)

| File | Purpose | Status |
|------|---------|--------|
| `pmm.rs` | Physical Memory Manager | ✅ Complete |
| `allocator.rs` | Page allocator | ✅ Complete |

### Object System (`object/`)

| File | Purpose | Status |
|------|---------|--------|
| `handle.rs` | Handle, Rights, HandleTable | ✅ Complete |
| `event.rs` | Event objects | ✅ Complete |
| `timer.rs` | Timer objects | ✅ Complete |
| `channel.rs` | IPC channels | ✅ Complete |
| `vmo.rs` | Virtual Memory Objects | ✅ Complete |
| `job.rs` | Job objects | ✅ Complete |

### Process Management (`process/`)

| File | Purpose | Status |
|------|---------|--------|
| `process.rs` | Process, Thread, AddressSpace | ✅ Complete |

### Synchronization (`sync/`)

| File | Purpose | Status |
|------|---------|--------|
| `spinlock.rs` | SpinLock implementation | ✅ Complete |
| `event.rs` | Event (SyncEvent) | ✅ Complete |
| `wait_queue.rs` | WaitQueue | ✅ Complete |

---

## Boot Sequence

### Phase 1: UEFI Boot (16-bit → 64-bit)

```
UEFI Firmware
    │
    ├─ Loads BOOTX64.EFI (rustux.efi)
    │
    ▼
rustux.efi Entry Point
    │
    ├─ [16-bit] bootstrap16.S
    │  └─ Set up temporary stack
    │
    ├─ [64-bit] Transition to long mode
    │
    ▼
main.rs::uefi_entry()
```

### Phase 2: Kernel Initialization

```
main.rs::uefi_entry()
    │
    ├─ Discover ACPI tables (RSDP)
    │
    ├─ Exit boot services
    │
    ▼
init.rs::kernel_init()
    │
    ├─ [1/5] Set up GDT
    │  └─ Configure code/data segments
    │
    ├─ [2/5] Set up IDT
    │  └─ Install exception handlers
    │
    ├─ [3/5] Install timer handler (vector 32)
    │
    ├─ [3.5/5] Install keyboard handler (vector 33)
    │
    ├─ [4/5] Initialize APIC
    │  └─ Enable LAPIC
    │
    ├─ [4.5/5] Configure keyboard IRQ (IRQ1 → Vector 33)
    │
    └─ [5/5] Configure timer (IRQ0 → Vector 32)
        └─ Start timer interrupts
```

### Phase 3: Runtime Mode

```
Kernel Runtime
    │
    ├─ Handle timer interrupts (periodic TICK)
    │
    ├─ Handle keyboard interrupts (on keypress)
    │
    ├─ Process system calls (when userspace exists)
    │
    └─ Schedule threads (when scheduler exists)
```

---

## Interrupt System

### Interrupt Routing

| Vector | Source | Handler | Status |
|--------|--------|---------|--------|
| 0-31 | Exceptions (x86) | `faults.rs` | ✅ Complete |
| 32 | IRQ0 (Timer) | `timer_handler` | ✅ Working |
| 33 | IRQ1 (Keyboard) | `keyboard_handler` | ✅ Installed |
| 34-47 | IRQ2-15 | `pic.rs` | 🔶 Configured |

### IDT Configuration

```rust
// Example: Installing timer handler
idt.set_gate(32, timer_handler as u64, 0x08, 0x8E);

// Example: Installing keyboard handler
idt.set_gate(33, keyboard_handler as u64, 0x08, 0x8E);
```

### APIC Configuration

```
Local APIC (LAPIC)
    │
    ├─ Base address: 0xFEE00000
    │
    ├─ Spurious Interrupt Vector Register
    │  └─ Enable APIC
    │
    ├─ Timer (LVT Timer)
    │  └─ Vector 32, periodic mode
    │
    └─ I/O APIC (for IRQ routing)
       ├─ IRQ0 → Vector 32 (Timer)
       └─ IRQ1 → Vector 33 (Keyboard)
```

---

## Memory Management

### Address Space Layout (AMD64)

| Region | Range | Purpose |
|--------|-------|---------|
| Kernel | `0xFFFF800000000000`+ | Kernel code/data |
| User | `0x0000000000000000`+ | User processes |
| Physical | `0x0` - `0x100000000` | Physical memory mapping |

### Memory Managers

| Component | Purpose | Status |
|-----------|---------|--------|
| PMM | Track free physical pages | 🔶 Stub |
| Allocator | Allocate/free pages | 🔶 Stub |
| Page Tables | Virtual → Physical mapping | ✅ AMD64 complete |

---

## Process Management

### Data Structures

```rust
pub struct Process {
    pub handle_table: HandleTable,
    pub address_space: AddressSpace,
    pub threads: Vec<Thread>,
}

pub struct Thread {
    pub state: ThreadState,
    pub registers: Registers,
    pub stack: usize,
}

pub struct AddressSpace {
    pub page_table: PageTable,
    pub regions: Vec<MemoryRegion>,
}
```

### Thread States

```
Created → Ready → Running → Blocked → Ready
                      ↓
                   Terminated
```

---

## System Calls

### System Call Interface (AMD64)

```asm
; System call via `syscall` instruction
mov rax, <syscall_number>
mov rdi, <arg1>
mov rsi, <arg2>
mov rdx, <arg3>
mov r10, <arg4>
mov r8,  <arg5>
mov r9,  <arg6>
syscall  ; Enters kernel at MSR_LSTAR
```

### Defined System Calls

| Number | Name | Purpose | Status |
|--------|------|---------|--------|
| 1 | `sys_handle_create` | Create kernel object | 🔶 Stub |
| 2 | `sys_handle_close` | Close handle | 🔶 Stub |
| 3 | `sys_handle_duplicate` | Duplicate handle | 🔶 Stub |
| 4 | `sys_vmo_create` | Create VMO | 🔶 Stub |
| 5 | `sys_vmo_read` | Read from VMO | 🔶 Stub |
| 6 | `sys_vmo_write` | Write to VMO | 🔶 Stub |
| 7 | `sys_process_create` | Create process | 🔶 Stub |
| 8 | `sys_thread_create` | Create thread | 🔶 Stub |
| 9 | `sys_channel_create` | Create IPC channel | 🔶 Stub |
| 10 | `sys_channel_read` | Read from channel | 🔶 Stub |
| 11 | `sys_channel_write` | Write to channel | 🔶 Stub |

---

## Kernel Objects

### Object Types

| Object | Description | Handle Rights |
|--------|-------------|---------------|
| `Vmo` | Virtual Memory Object | READ, WRITE, EXECUTE, MAP |
| `Process` | Process | READ, WRITE, ENUMERATE |
| `Thread` | Thread | READ, WRITE, SUSPEND, RESUME |
| `Event` | Event | READ, WRITE, SIGNAL |
| `Timer` | Timer | READ, WRITE, SIGNAL |
| `Channel` | IPC Channel | READ, WRITE |
| `Job` | Job (process group) | READ, WRITE, ENUMERATE |

### Handle Operations

```rust
// Create a handle
let handle: Handle = kernel_object.create_handle(Rights::READ | Rights::WRITE);

// Duplicate with fewer rights
let dup_handle = handle.duplicate(Rights::READ)?;

// Check rights before operation
if !handle.has_right(Rights::WRITE) {
    return Err(Error::AccessDenied);
}
```

---

## Device Drivers

### Currently Supported

| Device | Driver | IRQ | Status |
|--------|--------|-----|--------|
| UART (Serial) | `drivers/uart.rs` | N/A | ✅ Working |
| Keyboard | IRQ handler | 1 | ✅ Installed |
| Timer (PIT) | APIC timer | 0 | ✅ Working |

### Driver Architecture

```
Driver
    │
    ├─ Register with kernel
    │
    ├─ Set up interrupt handler
    │
    ├─ Create device node (optional)
    │
    └─ Provide ioctl interface (future)
```

---

## Current Limitations

### Known Limitations

1. **No Userspace Yet:** No process execution, all code runs in kernel mode
2. **No Scheduler:** Single-threaded execution, no preemption
3. **No Filesystem:** No storage drivers or VFS layer
4. **No Networking:** No network stack or drivers
5. **Stubbed Syscalls:** Syscall handlers exist but are not implemented
6. **Basic Memory Management:** PMM and allocator are stubs

### Planned Improvements

- [ ] Implement proper PMM with free page tracking
- [ ] Add scheduler with round-robin or priority-based scheduling
- [ ] Implement first userspace process
- [ ] Add filesystem (VFS + ext2 driver)
- [ ] Implement full syscall suite
- [ ] Add networking stack

---

## Build and Test

### Build Commands

```bash
# Build kernel (release)
cd /var/www/rustux.com/prod/rustux
cargo build --release --bin rustux --features uefi_kernel --target x86_64-unknown-uefi

# Build kernel (debug)
cargo build --bin rustux --features uefi_kernel --target x86_64-unknown-uefi

# Create bootable image
./build.sh
```

### Test Commands

```bash
# Test in QEMU (UEFI)
./test-qemu.sh

# Manual QEMU launch
qemu-system-x86_64 \
    -bios /usr/share/ovmf/OVMF.fd \
    -drive file=rustux.img,format=raw \
    -nographic \
    -device isa-debugcon,iobase=0xE9,chardev=debug \
    -chardev file,id=debug,path=/tmp/rustux-debug.log \
    -m 512M \
    -machine q35 \
    -smp 1
```

---

## References

- **Zircon Kernel Objects:** https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- **UEFI Specification:** https://uefi.org/specifications
- **AMD64 Manuals:** https://www.amd.com/en/developer/manuals
- **OSDev Wiki:** https://wiki.osdev.org/

---

*Last Updated: 2025-01-18*
*Author: Rustux Kernel Team*
*License: MIT*
