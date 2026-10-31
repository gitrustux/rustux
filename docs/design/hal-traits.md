# HAL (Hardware Abstraction Layer) Traits Specification

> **Design target.** Moved from `gitrustux/loader` (`docs/hal_traits_spec.md`). The kernel currently implements only the interrupt-controller part of this (`src/traits.rs`).

**Purpose:** Zero-cost, per-arch implementations providing architecture-specific primitives to the portable kernel core.

---

## Design Principles

1. **Zero-cost abstraction** - Traits compile to monomorphized implementations
2. **No dynamic dispatch** - All calls resolved at compile time
3. **No runtime branching** - `#[cfg(target_arch="...")]` gates
4. **Identical semantics** - Behavior same across architectures

---

## Core Traits

### ArchStartup

Boot and early initialization.

```rust
pub trait ArchStartup {
    /// Early initialization before MMU
    fn early_init() -> !;

    /// Initialize MMU and page tables
    fn init_mmu() -> Result<()>;

    /// Install exception/trap vectors
    fn init_exceptions() -> Result<()>;

    /// Timer initialization
    fn init_timer();

    /// Interrupt controller init
    fn init_interrupts();

    /// Secondary CPU (AP/hart) bring-up
    fn init_secondary(cpu_id: u32) -> !;
}
```

**Implementation notes:**
- ARM64: EL1/EL2 transition, TTBR setup, GIC init
- x86-64: Long mode, GDT/IDT, APIC init
- RISC-V: S-mode entry, SATP setup, PLIC init

---

### ArchThreadContext

Thread context save/restore.

```rust
pub trait ArchThreadContext: Sized {
    /// Context saved on trap/syscall entry
    type TrapFrame;

    /// Initialize for kernel thread entry
    fn init_kernel(entry: fn() -> !, stack: &KernelStack) -> Self;

    /// Initialize for user thread entry
    fn init_user(entry: u64, stack: u64, aspace: &AddressSpace) -> Self;

    /// Save current context to storage
    fn save_current(tf: &Self::TrapFrame) -> Self;

    /// Restore context and switch to it
    fn restore_and_switch(ctx: &Self) -> !;

    /// Get/set TLS base
    fn get_tls_base() -> usize;
    fn set_tls_base(base: usize);
}
```

**Register state minimum:**
- General purpose registers
- Program counter (PC/IP)
- Stack pointer (SP)
- Status/flags register
- Optional: FPU/SIMD state (lazy save)

---

### ArchMMU

Memory management unit operations.

```rust
pub trait ArchMMU {
    /// Page table entry type
    type PTE: Copy + Eq;

    /// Root page table type
    type PageTable;

    /// Create new root page table
    fn new_page_table() -> Result<Self::PageTable>;

    /// Map pages with attributes
    fn map(
        pt: &mut Self::PageTable,
        virt: VAddr,
        phys: PAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<()>;

    /// Unmap pages
    fn unmap(pt: &mut Self::PageTable, virt: VAddr, len: usize);

    /// Change protection
    fn protect(
        pt: &mut Self::PageTable,
        virt: VAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<()>;

    /// Resolve virtual to physical
    fn resolve(pt: &Self::PageTable, virt: VAddr) -> Option<PAddr>;

    /// Flush TLB entries
    fn flush_tlb(virt: Option<VAddr>);

    /// Flush TLB for specific ASID
    fn flush_tlb_asid(asid: Asid);

    /// Get/set root page table (switch address space)
    fn set_root_table(pt: &Self::PageTable) -> Result<()>;
    fn get_root_table() -> *const Self::PageTable;
}
```

**MapFlags (portable):**

| Flag | ARM64 | x86-64 | RISC-V |
|------|-------|--------|--------|
| READ | AP[2]=1 | R=1 | R=1 |
| WRITE | AP[2:1]=11 | W=1 | W=1 |
| EXECUTE | PXN=0 | NX=0 | X=1 |
| USER | AP[1]=1 | U=1 | U=1 |
| GLOBAL | nG=0 | G=1 | G=1 |

---

### ArchTimer

Timer and clock operations.

```rust
pub trait ArchTimer {
    /// Get monotonic clock (nanoseconds)
    fn now_monotonic() -> u64;

    /// Get realtime clock (nanoseconds, may slew)
    fn now_realtime() -> u64;

    /// Set one-shot deadline
    fn set_oneshot(deadline: u64);

    /// Set repeating timer
    fn set_repeating(interval: u64);

    /// Cancel timer
    fn cancel();

    /// Get timer resolution
    fn resolution() -> u64;

    /// Enable/disable timer interrupt
    fn enable_interrupt(enable: bool);
}
```

**Timer sources:**
- ARM64: ARM Generic Timer (CNTVCT)
- x86-64: TSC, HPET, LAPIC timer
- RISC-V: time CSR, stimecmp

---

### ArchInterrupts

Interrupt controller operations.

```rust
pub trait ArchInterrupts {
    /// Enable IRQ for this CPU
    fn enable_irq(irq: u32);

    /// Disable IRQ for this CPU
    fn disable_irq(irq: u32);

    /// Send end-of-interrupt
    fn end_of_interrupt(irq: u32);

    /// Configure IRQ trigger (edge/level)
    fn configure_irq(irq: u32, trigger: TriggerMode);

    /// Send IPI to target CPU
    fn send_ipi(target_cpu: u32, vector: u8);

    /// Get current IRQ number (in handler)
    fn current_irq() -> u32;

    /// Mask all interrupts
    fn mask_all();

    /// Unmask interrupts
    fn unmask();
}
```

**Interrupt controllers:**
- ARM64: GICv2/v3
- x86-64: APIC/IOAPIC
- RISC-V: PLIC + local interrupts

---

### ArchSynchronization

Atomic and barrier operations.

```rust
pub trait ArchSynchronization {
    /// Full memory barrier
    fn mb();

    /// SMP memory barrier
    fn smp_mb();

    /// Read memory barrier
    fn rmb();

    /// Write memory barrier
    fn wmb();

    /// Spin loop hint (pause/yield)
    fn spin_hint();

    /// Get cycle counter
    fn cycle_count() -> u64;

    /// Get instruction counter
    fn instret_count() -> u64;
}
```

**Instructions:**
- ARM64: `dmb`, `dsb`, `yield`, `mrs pmccntr_el0`
- x86-64: `mfence`, `pause`, `rdtsc`
- RISC-V: `fence`, `pause`, `rdcycle`

---

### ArchCache

Cache operations.

```rust
pub trait ArchCache {
    /// Data cache line size
    fn dcache_line_size() -> usize;

    /// Instruction cache line size
    fn icache_line_size() -> usize;

    /// Flush data cache range
    fn dcache_flush(vaddr: *const u8, len: usize);

    /// Invalidate data cache range
    fn dcache_invalidate(vaddr: *const u8, len: usize);

    /// Clean (flush) data cache range
    fn dcache_clean(vaddr: *const u8, len: usize);

    /// Flush instruction cache
    fn icache_flush(vaddr: *const u8, len: usize);

    /// Sync instruction and data cache
    fn idcache_sync(vaddr: *const u8, len: usize);
}
```

---

### ArchDebug

Debug and profiling support.

```rust
pub trait ArchDebug {
    /// Enable/disable hardware breakpoints
    fn set_breakpoint(addr: Option<VAddr>, kind: BreakpointKind) -> Result<()>;

    /// Enable single-step
    fn enable_single_step(enable: bool);

    /// Read register for debugging
    fn read_register(reg: RegId) -> u64;

    /// Write register for debugging
    fn write_register(reg: RegId, val: u64) -> Result<()>;

    /// Get current instruction pointer
    fn get_ip() -> u64;

    /// Get current stack pointer
    fn get_sp() -> u64;
}
```

---

## Implementation Files

### ARM64 (`/arch/arm64/`)

| Trait | Implementation |
|-------|----------------|
| ArchStartup | `start.S`, `boot_mmu.rs` |
| ArchThreadContext | `thread.rs`, `exceptions.S` |
| ArchMMU | `mmu.rs` |
| ArchTimer | `registers.rs` (CNTVCT) |
| ArchInterrupts | GIC driver |
| ArchSynchronization | `arch_ops.rs` |
| ArchCache | `cache-ops.S` |

### x86-64 (`/arch/x86/`)

| Trait | Implementation |
|-------|----------------|
| ArchStartup | `start.S`, `boot_mmu.rs` |
| ArchThreadContext | `thread.rs`, `entry.S` |
| ArchMMU | `mmu.rs` |
| ArchTimer | TSC/HPET driver |
| ArchInterrupts | APIC driver |
| ArchSynchronization | `arch_ops.rs` |
| ArchCache | WBINVD/CLFLUSH |

### RISC-V (`/arch/riscv64/`)

| Trait | Implementation |
|-------|----------------|
| ArchStartup | `start.S`, `boot_mmu.rs` |
| ArchThreadContext | `thread.rs`, `exceptions.S` |
| ArchMMU | `mmu.rs` |
| ArchTimer | `registers.rs` (time CSR) |
| ArchInterrupts | PLIC driver |
| ArchSynchronization | `arch_ops.rs` |
| ArchCache | `fence.i`, cbo.* |

---

## Usage Pattern

```rust
// Portable kernel code uses trait
use crate::arch::ArchMMU;

fn map_kernel_stack(stack: &KernelStack) -> Result<()> {
    let flags = MapFlags::READ | MapFlags::WRITE;
    ArchMMU::map(
        &mut kernel_page_table(),
        stack.vaddr(),
        stack.paddr(),
        stack.size(),
        flags,
    )
}
```

Compiled for ARM64 → calls `arm64::mmu::map`
Compiled for x86-64 → calls `x86::mmu::map`
Compiled for RISC-V → calls `riscv64::mmu::map`

---

*HAL Traits Spec v1 - 2025-01-03*
