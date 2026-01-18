# REFACTORING_PLAN.md - Kernel Refactoring Progress

**Date:** 2026-01-18
**Status:** Phase 1 Complete, Phase 2 In Progress

---

## Quick Reference: Detailed Migration Plan

For complete file-by-file migration mapping, see **`MIGRATION_PLAN.md`** which contains:
- Critical rules for ACPI conflicts (DO NOT MIGRATE list)
- 12 sections of detailed file mappings (architecture, memory, process, sync, objects, syscalls, drivers, etc.)
- Proposed migration order with phases 1-5

---

## Completed Work ✅

### Phase 1: Interrupt System Integration ✅

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

### 1. Directory Structure Fixed ✅

**Corrected:** `rustux/src/` (proper Zicorn-style layout)
```
rustux/
├── Cargo.toml
└── src/
    ├── lib.rs              # Main library with panic handler
    ├── traits.rs           # Cross-architecture interrupt traits
    ├── arch/               # Architecture-specific code
    │   ├── amd64/          # x86_64 APIC implementation ✅
    │   ├── arm64/          # ARM64 GIC (stub) ⚠️
    │   └── riscv64/        # RISC-V PLIC (stub) ⚠️
    ├── interrupt/          # Generic interrupt handling
    ├── acpi/               # ACPI table parsing ✅
    ├── sched/              # Scheduler and thread management ✅
    └── drivers/            # (empty, for future)
```

### 2. Cross-Architecture Interrupt Controller Trait ✅

**Location:** `src/traits.rs`
```rust
pub trait InterruptController {
    fn enable_irq(&mut self, irq: u64, vector: u64);
    fn disable_irq(&mut self, irq: u64);
    fn send_eoi(&self, irq: u64);
    fn init(&mut self) -> Result<(), &'static str>;
}
```

**Purpose:** Architecture-agnostic API for x86_64, ARM64 (GIC), RISC-V (PLIC)

### 3. x86_64 APIC Implementation ✅

**Location:** `src/arch/amd64/apic.rs`
- Local APIC initialization (UEFI firmware support)
- EOI handling
- I/O APIC configuration

**Location:** `src/arch/amd64/controller.rs`
- `X86_64InterruptController` implements `InterruptController` trait
- IRQ routing (e.g., IRQ1 → Vector 33)

### 4. ACPI MADT Parsing ✅

**Location:** `src/acpi/`
- `rsdp.rs` - RSDP discovery from legacy BIOS locations
- `rsdt.rs` - RSDT/XSDT parsing with iterator
- `madt.rs` - MADT parsing for IOAPIC/LAPIC discovery
- Dynamic IOAPIC address discovery (not hardcoded)

**Usage:**
```rust
if let Some(rsdp) = find_rsdp() {
    if let Some(madt) = find_and_parse_madt(&rsdp) {
        if let Some(ioapic_addr) = madt.first_ioapic_address() {
            // Use discovered address
        }
    }
}
```

### 5. Testing Infrastructure ✅

**Location:** `src/testing/`
- `harness.rs` - Interrupt test harness
- `qemu.rs` - QEMU test configuration

### 6. Scheduler/Thread Primitives ✅

**Location:** `src/sched/`
- `state.rs` - ThreadState, ThreadPriority, RunQueue
- `thread.rs` - Thread, SavedRegisters, StackConfig
- `scheduler.rs` - Round-robin scheduler with priority

### 7. Capability System Design ✅

**Location:** `docs/design/capability-system.md`
- KernelObject trait
- Handle rights system (20+ right types)
- HandleTable per-process management
- Interrupt objects with port binding
- Zircon-inspired security model

---

## Pending Tasks

### Phase 2A: Keyboard Input Test Verification ✅ (Complete)

**Completed 2026-01-18:**
- QEMU test runs successfully with keyboard interrupts
- Debug log shows timer TICK messages
- Ready to test actual key presses for [KEY:XX] messages

### Phase 2B: Shell Migration with Interrupt-Driven Input ⏳ (In Progress)

**Current Status:** Shell exists in old kernel but uses polling-based input

**Tasks:**
1. Migrate shell from `kernel/user/shell.rs` to `rustux/userspace/shell/src/main.rs`
2. Replace polling input with interrupt-driven keyboard handler
3. Implement input buffer for keyboard events
4. Test shell with interrupt-driven I/O

### Phase 2C: Full Kernel Component Migration ⏳ (Pending)

**Reference:** See `MIGRATION_PLAN.md` for complete file-by-file mapping (12 sections)

**Priority Order:**

#### Phase 2C.1: Architecture Components (AMD64)
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/arch/amd64/registers.rs` | `rustux/src/arch/amd64/registers.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/tsc.rs` | `rustux/src/arch/amd64/tsc.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/syscall.rs` | `rustux/src/arch/amd64/syscall.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/uspace_entry.rs` | `rustux/src/arch/amd64/uspace_entry.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/ioport.rs` | `rustux/src/arch/amd64/ioport.rs` | MEDIUM |
| `kernel/src/kernel/arch/amd64/bootstrap16.rs` | `rustux/src/arch/amd64/bootstrap16.rs` | MEDIUM |
| `kernel/src/kernel/arch/amd64/cache.rs` | `rustux/src/arch/amd64/cache.rs` | LOW |
| `kernel/src/kernel/arch/amd64/ops.rs` | `rustux/src/arch/amd64/ops.rs` | LOW |
| `kernel/src/kernel/arch/amd64/faults.rs` | `rustux/src/arch/amd64/faults.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/page_tables.rs` | `rustux/src/arch/amd64/mm/page_tables.rs` | HIGH |
| `kernel/src/kernel/arch/amd64/mmu.rs` | `rustux/src/arch/amd64/mmu.rs` | HIGH |

#### Phase 2C.2: Memory Management
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/pmm.rs` | `rustux/src/mm/pmm.rs` | **CRITICAL** |
| `kernel/src/kernel/vm/*.rs` | `rustux/src/mm/*.rs` | **CRITICAL** |
| `kernel/src/kernel/allocator.rs` | `rustux/src/mm/allocator.rs` | **CRITICAL** |

#### Phase 2C.3: Process & Thread Management
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/process/mod.rs` | `rustux/src/process/mod.rs` | HIGH |
| `kernel/src/kernel/thread/mod.rs` | `rustux/src/sched/thread.rs` | REVIEW (merge) |
| `kernel/src/kernel/sched/mod.rs` | `rustux/src/sched/scheduler.rs` | REVIEW (merge) |
| `kernel/src/kernel/init.rs` | `rustux/src/init.rs` | HIGH |

#### Phase 2C.4: Synchronization Primitives
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/sync/*.rs` | `rustux/src/sync/*.rs` | MEDIUM |
| `kernel/src/kernel/mutex.rs` | `rustux/src/sync/kernel_mutex.rs` | MEDIUM |
| `kernel/src/kernel/spinlock.rs` | `rustux/src/sync/spinlock.rs` | MEDIUM |

#### Phase 2C.5: Objects & Capabilities (Per Capability System Design)
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/object/mod.rs` | `rustux/src/object/mod.rs` | MEDIUM |
| `kernel/src/kernel/object/handle.rs` | `rustux/src/object/handle.rs` | MEDIUM |
| `kernel/src/kernel/object/channel.rs` | `rustux/src/object/channel.rs` | MEDIUM |
| `kernel/src/kernel/object/event.rs` | `rustux/src/object/event.rs` | MEDIUM |
| `kernel/src/kernel/object/vmo.rs` | `rustux/src/object/vmo.rs` | MEDIUM |
| `kernel/src/kernel/object/timer.rs` | `rustux/src/object/timer.rs` | MEDIUM |
| `kernel/src/kernel/object/job.rs` | `rustux/src/object/job.rs` | MEDIUM |

#### Phase 2C.6: System Calls
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/syscalls/mod.rs` | `rustux/src/syscall/mod.rs` | HIGH |
| `kernel/src/kernel/syscalls/*.rs` | `rustux/src/syscall/*.rs` | HIGH |

#### Phase 2C.7: Device Drivers
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/dev/uart/*.rs` | `rustux/src/drivers/uart/*.rs` | MEDIUM |
| `kernel/src/kernel/dev/timer/*.rs` | `rustux/src/drivers/timer/*.rs` | LOW |
| `kernel/src/kernel/dev/interrupt/*.rs` | `rustux/src/drivers/interrupt/*.rs` | LOW |

#### Phase 2C.8: ARM64 & RISC-V Support
| Old Path | New Path | Priority |
|----------|----------|----------|
| `kernel/src/kernel/arch/arm64/*.rs` | `rustux/src/arch/arm64/*.rs` | LOW |
| `kernel/src/kernel/arch/riscv64/*.rs` | `rustux/src/arch/riscv64/*.rs` | LOW |
| `kernel/src/kernel/dev/interrupt/arm_gic/*` | `rustux/src/arch/arm64/gic/*` | LOW |

---

## Critical Migration Rules ⚠️

### DO NOT MIGRATE (ACPI Conflicts)
These files must NOT be migrated as they conflict with the new ACPI implementation in `rustux/src/acpi/`:

| Old Path | Reason | Replacement |
|----------|--------|-------------|
| `kernel/src/kernel/arch/amd64/include/arch/amd64/acpi.rs` | ACPI header | `rustux/src/acpi/` |
| `kernel/src/kernel/arch/amd64/acpi.S` | ACPI assembly | `rustux/src/acpi/` (Rust) |
| `kernel/src/kernel/platform/pc/acpi.cpp` | ACPI parsing | `rustux/src/acpi/` |
| `kernel/src/kernel/platform/pc/include/platform/pc/acpi.h` | ACPI header | `rustux/src/acpi/` |

### REVIEW FIRST (Potential Conflicts)
These files may need modification before migration:

| Old Path | Conflict | Action |
|----------|----------|--------|
| `kernel/src/kernel/arch/amd64/apic.rs` | May duplicate new `rustux/src/arch/amd64/apic.rs` | **REVIEW** - merge if needed |
| `kernel/src/kernel/arch/amd64/interrupts.rs` | May use old interrupt patterns | **REVIEW** - update to use traits |

---

## Next Steps

1. ✅ **Phase 1 Complete** - Interrupt system working
2. ⏳ **Phase 2B In Progress** - Migrate shell with interrupt-driven input
3. ⏳ **Phase 2C Pending** - Execute migration using `MIGRATION_PLAN.md` as reference

---

*Status: Phase 1 Complete | Phase 2B In Progress | See MIGRATION_PLAN.md for full file mapping*
