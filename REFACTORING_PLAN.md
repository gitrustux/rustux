# Kernel Refactoring Plan - Clean Architecture

**Date:** 2025-01-17
**Status:** Planning Phase - Awaiting IRQ stabilization before full implementation

---

## Problem Statement

The current kernel structure has ownership confusion:
- `kernel-efi/` contains kernel + platform glue + APIC (too much responsibility)
- Duplicate interrupt handling between `kernel-efi/runtime.rs` and `src/kernel/arch/amd64/apic.rs`
- No clear separation between firmware-specific code and architecture-specific code
- Documentation scattered across multiple locations

## Target Structure

```
rustux/
├── kernel/              # Core kernel (firmware-agnostic)
│   ├── arch/
│   │   ├── amd64/
│   │   │   ├── apic.rs           # Local/IO APIC (ONLY APIC code)
│   │   │   ├── interrupts.rs    # Interrupt wrappers
│   │   │   ├── idt.rs           # IDT structures
│   │   │   ├── gdt.rs           # GDT structures
│   │   │   └── ...
│   │   ├── arm64/
│   │   │   ├── gic.rs           # GIC interrupt controller
│   │   │   └── ...
│   │   └── riscv64/
│   │       ├── plic.rs          # PLIC interrupt controller
│   │       └── ...
│   ├── drivers/
│   │   ├── keyboard/
│   │   │   ├── ps2.rs           # PS/2 keyboard driver
│   │   │   └── ...
│   │   ├── framebuffer/
│   │   │   └── vga.rs           # VGA framebuffer driver
│   │   └── ...
│   ├── interrupt/
│   │   ├── idt.rs              # Architecture-agnostic IDT
│   │   ├── controller.rs       # Interrupt controller trait
│   │   └── pic.rs              # Legacy PIC (if needed)
│   ├── sched/
│   ├── mm/
│   ├── process/
│   └── lib.rs                   # Kernel library entry point
│
├── bootloader/        # Bootloaders (NOT part of kernel)
│   ├── uefi/
│   │   ├── main.rs             # EFI entry point ONLY
│   │   ├── efi_console.rs      # UEFI console output
│   │   ├── efi_memory.rs       # UEFI memory map processing
│   │   ├── efi_system.rs       # UEFI system table access
│   │   └── jump_to_kernel.rs   # Jump to kernel entry point
│   ├── limine/
│   └── multiboot/
│
├── installer/         # Linux-based installer
│   └── (separate repo)
│
├── apps/              # Userspace programs
│   ├── coreutils/
│   ├── shell/
│   ├── init/
│   ├── services/
│   └── libc/
│
└── docs/              # Single source of truth for documentation
    ├── architecture/
    ├── boot/
    ├── interrupts/
    ├── memory/
    └── decisions/         # Architectural Decision Records (ADRs)
```

---

## Phase 1: Interrupt System Cleanup (DO THIS FIRST - After IRQ Works)

**Goal:** Consolidate all interrupt code into proper locations

### Current State (Problematic)

```
kernel-efi/src/runtime.rs (54KB!):
  ├── IdtEntry, IdtPointer structures
  ├── Exception handling system
  ├── LocalApicRegisters structure
  ├── InterruptController implementation
  ├── Keyboard IRQ1 handler
  └── PIC functions (init_pic, etc.)

src/kernel/arch/amd64/apic.rs:
  └── APIC stubs (TODOs)
```

### Action Items

1. **Move IDT/Exception handling** from `runtime.rs` → `kernel/arch/amd64/idt.rs`
2. **Consolidate APIC implementations** between `runtime.rs` and `arch/amd64/apic.rs`
3. **Create interrupt controller trait** in `kernel/interrupt/`
4. **Make keyboard driver interrupt-agnostic** - remove APIC knowledge from it

### Files to Modify

| From | To | Description |
|------|-----|-------------|
| `kernel-efi/src/runtime.rs` (IDT/Exception parts) | `src/kernel/arch/amd64/idt.rs` | Move IDT structures |
| `kernel-efi/src/runtime.rs` (APIC parts) | `src/kernel/arch/amd64/apic.rs` | Consolidate APIC |
| `kernel-efi/src/runtime.rs` (keyboard IRQ) | `src/kernel/drivers/keyboard/` | Move handler |
| `kernel-efi/src/runtime.rs` (PIC parts) | `src/kernel/arch/amd64/pic.rs` | Move PIC if needed |

---

## Phase 2: Reduce kernel-efi to Platform Adapter

**Goal:** Make kernel-efi ONLY handle UEFI bootstrapping

### What kernel-efi/ Should Become

```
kernel-efi/
├── Cargo.toml
├── src/
│   ├── main.rs                 # EFI entry point
│   ├── efi_console.rs          # UEFI console
│   ├── efi_memory.rs           # Process UEFI memory map
│   ├── efi_system.rs           # Access UEFI system table
│   ├── efi_boot.rs             # ExitBootServices handling
│   ├── efi_loader.rs           # Load kernel from disk
│   └── jump_to_kernel.rs       # Jump to kernel entry
└── build.rs
```

### What Should Be Removed from kernel-efi

- ❌ All exception handling (move to kernel/arch/)
- ❌ All APIC/IOAPIC code (move to kernel/arch/)
- ❌ All keyboard driver code (move to kernel/drivers/)
- ❌ All framebuffer code (move to kernel/drivers/)
- ❌ Memory allocator (move to kernel/mm/)
- ❌ Process management (move to kernel/process/)

---

## Phase 3: Documentation Consolidation

**Goal:** Single source of truth for all documentation

### Current Locations (Scattered)

```
rustux.com/html/rustica/docs/
/var/www/rustux.com/prod/FIXED.md
/var/www/rustux.com/prod/TREE.md
/var/www/rustux.com/prod/TODO.md
/var/www/rustux.com/prod/kernel/README.md
```

### Target Structure

```
docs/
├── architecture/
│   ├── interrupt-system.md    # APIC/GIC/PLIC architecture
│   ├── memory-layout.md
│   └── boot-sequence.md
├── boot/
│   ├── uefi.md               # UEFI boot protocol
│   ├── limine.md
│   └── multiboot.md
├── interrupts/
│   ├── apic.md               # Local/IO APIC
│   ├── gic.md                # ARM64 GIC
│   └── plic.md               # RISC-V PLIC
├── decisions/
│   ├── 001-uefi-kernel-separation.md
│   ├── 002-interrupt-ownership.md
│   └── 003-apic-routing-uefi.md
└── roadmap.md
```

---

## Phase 4: Application Boundary

**Goal:** Clear userspace/kernel separation

### apps/ Structure

```
apps/
├── coreutils/         # ls, cat, echo, etc.
├── shell/             # Interactive shell
├── init/              # PID 1
├── services/          # Background services
└── libc/              # System call interface
```

### syscall ABI

```
abi/
├── syscalls.rs         # System call numbers/definitions
├── handles.rs         # Resource handle types
└── messages.rs        # Kernel↔Userspace messages
```

---

## Implementation Priority

Given we're still debugging IRQ issues, **implementation order is:**

### 🔴 Critical Path (After IRQ Works)

1. **Consolidate APIC code** - Remove duplication between `runtime.rs` and `apic.rs`
2. **Create interrupt controller trait** - Clean API for interrupt routing
3. **Make keyboard driver interrupt-agnostic** - No direct APIC access

### 🟡 Important (Next)

4. **Reduce kernel-efi to platform adapter** - Move non-EFI code out
5. **Create proper syscall ABI** - Separate from kernel internals

### 🟢 Can Wait (Later)

6. **Documentation consolidation** - Single docs/ directory
7. **Full repo restructuring** - rustux/, bootloader/, etc.

---

## Key Architectural Decisions (ADRs)

### ADR-001: UEFI Kernel Separation

**Status:** Pending

**Context:** kernel-efi currently contains kernel + platform glue

**Decision:** kernel-efi MUST only contain UEFI bootstrapping code. All kernel code lives in `kernel/`.

**Rationale:**
- UEFI is a loader, not part of the kernel
- Kernel should be firmware-agnostic
- Enables multiple bootloader support (Limine, GRUB, etc.)

### ADR-002: Interrupt Ownership

**Status:** Pending

**Context:** Multiple files handle interrupts with duplication

**Decision:** Only `src/kernel/arch/<arch>/interrupts.rs` owns interrupt controller code.

**Rationale:**
- Single source of truth for interrupt handling
- Architecture-specific optimizations in one place
- Clear boundary between generic kernel and arch code

### ADR-003: APIC Routing Under UEFI

**Status:** Partially Implemented

**Context:** UEFI firmware routes IRQs through IOAPIC → Local APIC

**Decision:** Kernel configures IOAPIC redirection; assumes LAPIC enabled by firmware.

**Rationale:**
- UEFI firmware initializes LAPIC during boot
- Re-enabling causes conflicts
- Firmware already set correct routing

---

## Migration Path

### Step 1: Create New Directory Structure

```bash
mkdir -p kernel/src/arch/amd64/interrupt
mkdir -p kernel/src/drivers/keyboard
mkdir -p kernel/src/drivers/framebuffer
mkdir -p kernel/interrupt
```

### Step 2: Move APIC Code

```bash
# Extract APIC parts from runtime.rs
# Consolidate with src/kernel/arch/amd64/apic.rs
```

### Step 3: Create Interrupt Controller Trait

```rust
// kernel/interrupt/controller.rs
pub trait InterruptController {
    fn enable_irq(&mut self, irq: u8, vector: u8);
    fn disable_irq(&mut self, irq: u8);
    fn send_eoi(&self, irq: u8);
}
```

### Step 4: Update kernel-efi

```rust
// kernel-efi/src/main.rs becomes:
fn efi_main() -> ! {
    // Exit boot services
    // Load kernel
    // Jump to kernel entry
}
```

---

## Testing Strategy

1. **Current Issue:** IRQ1 not working on real hardware
2. **Blocker:** Cannot refactor until IRQ is stable
3. **Prerequisite:** Working keyboard interrupt delivery
4. **Approach:** Minimal changes first, validate, then refactor

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Breaking working code | Extensive testing after each phase |
| Introducing new bugs | Keep old structure during migration |
| Merge conflicts | Clear branch per phase, systematic migration |
| Documentation drift | Update docs as part of each phase |

---

## Next Steps

**WAIT for IRQ to work on real hardware first**, then:

1. Implement Phase 1 (Interrupt System Cleanup)
2. Test and validate
3. Implement Phase 2 (Reduce kernel-efi)
4. Implement Phase 3 (Docs consolidation)
5. Full repository restructuring

---

*This plan will be updated as we progress through refactoring.*
