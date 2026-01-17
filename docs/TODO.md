# Diagnose Keyboard IRQ1 Not Firing in UEFI x86_64 Kernel (APIC-only)

## Context

You are working on a UEFI x86_64 kernel.
- `[POLLING]` means IRQ1 is not being delivered
- The kernel boots via UEFI, not BIOS
- Legacy PIC IRQ delivery is NOT viable under UEFI

## Modern UEFI Interrupt Routing

```
Keyboard → IOAPIC → Local APIC → CPU
```

**Hard Constraints:**
- ❌ Do NOT disable the Local APIC (prevents all hardware IRQs)
- ❌ Do NOT rely on legacy PIC
- ❌ Do NOT suggest hlt loops as a fix
- ✅ Use IOAPIC + LAPIC or explain exactly why they are not wired
- `sti` is already present and not the root cause

---

# DIAGNOSTIC RESULTS

## Step 1 — Inspect the EFI kernel path only ✅

**Files examined:**
- `kernel-efi/src/main.rs`
- `kernel-efi/src/runtime.rs`

**Findings:**
- Code currently uses legacy PIC (`init_pic()`)
- Code currently DISABLING Local APIC at runtime.rs:543
- NO IOAPIC initialization present
- NO ACPI/MADT parsing present

## Step 2 — Identify APIC state ✅

**Current state:**
| Component | Status | Details |
|-----------|--------|---------|
| Local APIC | ❌ DISABLED by code at runtime.rs:543 | `apic_svr.write_volatile(0xFF)` clears bit 8 (enable) |
| IOAPIC | ❌ NOT initialized | No `apic_io_init()` call exists in UEFI path |
| ACPI MADT | ❌ NOT parsed | No MADT table parsing to discover IOAPIC base address |
| IRQ1 routing | ❌ BROKEN | Code tries PIC-only, but UEFI uses APIC routing |

**Why PIC doesn't work under UEFI:**
Modern UEFI firmware (like OVMF) routes ALL hardware interrupts through IOAPIC, not the legacy 8259 PIC. Even if the PIC is programmed, the hardware keyboard interrupts go to IOAPIC, which forwards to Local APIC. With Local APIC disabled, interrupts are lost.

## Step 3 — Trace IRQ1 end-to-end ✅

**Current broken path:**
```
Keyboard → IOAPIC → [Local APIC DISABLED] → ❌ BLOCKED
                  (or to PIC which is ignored by UEFI firmware)
```

**Current code expectations (WRONG):**
- Expects IRQ1 on IDT vector 33 (32 + 1)
- IDT[33] has `keyboard_irq_stub` installed ✓
- PIC remapped to vectors 32-47 ✓
- PIC IRQ1 unmasked ✓
- `sti` executed ✓

**Why IRQ1 never fires:**
- UEFI firmware routes keyboard to IOAPIC, not PIC
- IOAPIC is NOT initialized, so IRQ1 not forwarded to Local APIC
- Local APIC is DISABLED, so even if IOAPIC worked, CPU wouldn't receive IRQ

**Correct path needed:**
```
Keyboard → IOAPIC (IRQ1 → Global IRQ) → Local APIC (Global IRQ → Vector) → CPU
           [Need: init]               [Need: ENABLED]              [IDT entry]
```

## Step 4 — Minimal, Correct Fix ✅

### Root Cause

The kernel disables Local APIC and relies on legacy PIC, but UEFI firmware routes hardware interrupts through IOAPIC→Local APIC exclusively. With Local APIC disabled, IRQ1 cannot reach the CPU regardless of PIC state.

### Exact Missing Initialization Steps

1. **DO NOT disable Local APIC** - Remove the `apic_svr.write_volatile(0xFF)` at runtime.rs:543
2. **Enable Local APIC properly** - Use existing `InterruptController::enable()` or write proper LAPIC enable
3. **Initialize IOAPIC** - Map IOAPIC MMIO region, configure IRQ1 redirection
4. **Route IRQ1 through IOAPIC** - Configure IOAPIC to forward keyboard IRQ to Local APIC

### Code-Level Fix

**File: `kernel-efi/src/runtime.rs`**

**Step 1: Remove LAPIC disable (lines 526-543)**
```rust
// DELETE THIS CODE:
// const LOCAL_APIC_BASE: u64 = 0xFEE0_0000;
// const SVR_OFFSET: usize = 0x70;
// let apic_svr = (LOCAL_APIC_BASE + SVR_OFFSET as u64) as *mut u32;
// apic_svr.write_volatile(0xFF);  // This disables LAPIC - WRONG!
```

**Step 2: Add IOAPIC and proper LAPIC initialization**

Add after the disabled section is removed:

```rust
/// Initialize keyboard interrupt handler for UEFI (APIC mode)
pub unsafe fn init_keyboard_interrupts() -> Result<(), &'static str> {
    // ================================================================
    // UEFI APIC INTERRUPT SETUP
    // ================================================================
    // UEFI firmware uses IOAPIC + Local APIC for interrupt routing.
    // Legacy PIC is NOT functional under UEFI.

    // 1. ENABLE LOCAL APIC
    const LOCAL_APIC_BASE: u64 = 0xFEE0_0000;
    const LAPIC_SVR_OFFSET: usize = 0x70;  // Spurious Vector Register
    const LAPIC_TPR_OFFSET: usize = 0x30;  // Task Priority Register

    let lapic_svr = (LOCAL_APIC_BASE + LAPIC_SVR_OFFSET as u64) as *mut u32;
    let lapic_tpr = (LOCAL_APIC_BASE + LAPIC_TPR_OFFSET as u64) as *mut u32;

    // Enable Local APIC (set bit 8 of SVR), set spurious vector to 0xFF
    lapic_svr.write_volatile(0x100 | 0xFF);

    // Set TPR to 0 (allow all interrupts)
    lapic_tpr.write_volatile(0);

    // 2. INITIALIZE IOAPIC (Standard UEFI IOAPIC address)
    const IOAPIC_BASE: u64 = 0xFEC0_0000;
    const IOAPIC_IOREGSEL: u64 = 0x00;
    const IOAPIC_IOWIN: u64 = 0x10;
    const IOAPIC_IRQ1_REDTBL: u8 = 0x12;  // Redirection table for IRQ1 (2 * 1 = 2, offset 0x10 + 2*4 = 0x12)

    let ioapic_sel = (IOAPIC_BASE + IOAPIC_IOREGSEL) as *mut u32;
    let ioapic_win = (IOAPIC_BASE + IOAPIC_IOWIN) as *mut u32;

    // Configure IOAPIC IRQ1 redirection entry
    // Format: [63:32] = (destination << 24) | (mask << 16) | (trigger << 15) | (polarity << 13) | (delivery << 8) | vector
    //         [31:0]  = (mask << 16) | (trigger << 15) | (polarity << 13) | (delivery << 8) | vector

    // Vector 33 for IRQ1 (keyboard)
    const IRQ1_VECTOR: u8 = 33;
    // Fixed delivery mode
    const DELIVERY_FIXED: u32 = 0 << 8;
    // Edge trigger (keyboard is edge-triggered)
    const TRIGGER_EDGE: u32 = 0 << 15;
    // Active high
    const POLARITY_HIGH: u32 = 0 << 13;
    // NOT masked (enabled)
    const MASK_UNMASKED: u32 = 0 << 16;

    let low_dword = DELIVERY_FIXED | TRIGGER_EDGE | POLARITY_HIGH | MASK_UNMASKED | (IRQ1_VECTOR as u32);
    let high_dword = 0u32;  // Destination = 0 (all CPUs, BSP will handle)

    // Write to IOAPIC redirection table for IRQ1 (entry 1, which is at offsets 0x12 and 0x13)
    ioapic_sel.write_volatile(0x12);  // Select IRQ1 low dword register
    ioapic_win.write_volatile(low_dword);

    ioapic_sel.write_volatile(0x13);  // Select IRQ1 high dword register
    ioapic_win.write_volatile(high_dword);

    // VISUAL CONFIRMATION: IOAPIC init complete
    const VGA_BUFFER: u64 = 0xB8000;
    let vga = VGA_BUFFER as *mut u16;
    let msg = b"IOAPIC!";
    for (i, &byte) in msg.iter().enumerate() {
        *vga.add(60 + i) = 0x0E00 | (byte as u16);
    }

    // 3. SET UP IDT ENTRY FOR IRQ1
    let kernel_cs: u16;
    core::arch::asm!(
        "mov {0:x}, cs",
        out(reg) kernel_cs,
        options(nomem, nostack, preserves_flags)
    );

    // Set up IDT entry for IRQ1 (keyboard) at vector 33
    let keyboard_handler = keyboard_irq_stub as *const () as u64;
    IDT[33] = IdtEntry::interrupt_gate(keyboard_handler, kernel_cs);

    // Reload IDT
    let idt_ptr = IdtPointer::new(
        &IDT as *const _ as u64,
        (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16
    );
    load_idt(&idt_ptr);

    // Initialize keyboard driver
    crate::keyboard::init();

    Ok(())
}
```

**Step 3: Update keyboard IRQ handler to send EOI to Local APIC**

File: `kernel-efi/src/runtime.rs` - `keyboard_irq_stub()` function (around line 354):

```rust
#[unsafe(naked)]
unsafe extern "C" fn keyboard_irq_stub() -> ! {
    core::arch::naked_asm!(
        // Save all general-purpose registers
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Call the keyboard handler
        "call {handler}",
        // Restore registers
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        // Send EOI to Local APIC (offset 0x40)
        "mov rcx, 0xFEE0_0040",  // Local APIC EOI register
        "mov dword ptr [rcx], 0",
        // Return from interrupt
        "iretq",
        handler = sym crate::keyboard::keyboard_irq_handler
    );
}
```

### Why This Works Under UEFI

1. **Local APIC enabled**: UEFI firmware expects Local APIC to be enabled for interrupt delivery. By setting SVR bit 8, we enable it.

2. **IOAPIC routing**: At address `0xFEC0_0000`, the IOAPIC receives hardware IRQs and forwards them to Local APIC. We configure IRQ1's redirection table entry to send keyboard interrupts to vector 33.

3. **IDT entry**: Vector 33 in the IDT points to `keyboard_irq_stub`, which handles the interrupt and sends EOI to Local APIC.

4. **Standard addresses**: `0xFEE0_0000` (Local APIC) and `0xFEC0_0000` (IOAPIC) are x86_64 standard addresses defined by the specification. UEFI firmware configures these at boot.

5. **No ACPI needed**: For basic keyboard operation, we can assume standard IOAPIC address. MADT parsing would be needed for multi-IOAPIC systems but is overkill for this fix.

---

## Summary

**Root Cause**: Local APIC disabled + IOAPIC not initialized = IRQ1 lost in UEFI

**Fix**: Enable Local APIC, initialize IOAPIC at 0xFEC0_0000, configure IRQ1 redirection

**Expected result**: IRQ1 fires, `[POLLING]` disappears, keyboard works via interrupts
