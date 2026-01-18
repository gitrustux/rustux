// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 APIC (Advanced Programmable Interrupt Controller) Implementation
//!
//! This module provides the actual APIC implementation for x86_64,
//! including Local APIC and I/O APIC support.

/// Local APIC MMIO register offsets
#[repr(C)]
pub struct LocalApicRegisters {
    _reserved0: [u32; 2],           // 0x00-0x07
    id: u32,                        // 0x08 - Local APIC ID
    _reserved1: [u32; 3],           // 0x0C-0x17
    version: u32,                   // 0x1C - Local APIC Version
    _reserved2: [u32; 4],           // 0x20-0x2F
    tpr: u32,                       // 0x30 - Task Priority Register
    _reserved3: [u32; 3],           // 0x34-0x3F
    eoi: u32,                       // 0x40 - EOI Register
    _reserved4: [u32; 3],           // 0x44-0x4F
    ldr: u32,                       // 0x50 - Logical Destination Register
    _reserved5: [u32; 3],           // 0x54-0x5F
    dfr: u32,                       // 0x60 - Destination Format Register
    _reserved6: [u32; 3],           // 0x64-0x6F
    svr: u32,                       // 0x70 - Spurious Interrupt Vector Register
    _reserved7: [u32; 3],           // 0x74-0x7F
    isr0: u32,                      // 0x80 - In-Service Register 0
    isr1: u32,                      // 0x84 - In-Service Register 1
    isr2: u32,                      // 0x88 - In-Service Register 2
    isr3: u32,                      // 0x8C - In-Service Register 3
    isr4: u32,                      // 0x90 - In-Service Register 4
    isr5: u32,                      // 0x94 - In-Service Register 5
    isr6: u32,                      // 0x98 - In-Service Register 6
    isr7: u32,                      // 0x9C - In-Service Register 7
    tmr0: u32,                      // 0xA0 - Trigger Mode Register 0
    tmr1: u32,                      // 0xA4 - Trigger Mode Register 1
    tmr2: u32,                      // 0xA8 - Trigger Mode Register 2
    tmr3: u32,                      // 0xAC - Trigger Mode Register 3
    tmr4: u32,                      // 0xB0 - Trigger Mode Register 4
    tmr5: u32,                      // 0xB4 - Trigger Mode Register 5
    tmr6: u32,                      // 0xB8 - Trigger Mode Register 6
    tmr7: u32,                      // 0xBC - Trigger Mode Register 7
    irr0: u32,                      // 0xC0 - Interrupt Request Register 0
    irr1: u32,                      // 0xC4 - Interrupt Request Register 1
    irr2: u32,                      // 0xC8 - Interrupt Request Register 2
    irr3: u32,                      // 0xCC - Interrupt Request Register 3
    irr4: u32,                      // 0xD0 - Interrupt Request Register 4
    irr5: u32,                      // 0xD4 - Interrupt Request Register 5
    irr6: u32,                      // 0xD8 - Interrupt Request Register 6
    irr7: u32,                      // 0xDC - Interrupt Request Register 7
    error_status: u32,              // 0xE0 - Error Status Register
    _reserved8: [u32; 5],           // 0xE4-0xF7
    icr_low: u32,                   // 0xF0 - Interrupt Command Register Low
    icr_high: u32,                  // 0xF4 - Interrupt Command Register High
    _reserved9: [u32; 2],           // 0xF8-0xFF
    timer_initial: u32,             // 0x170 - Timer Initial Count
    timer_current: u32,             // 0x180 - Timer Current Count
    _reserved10: [u32; 2],          // 0x190-0x197
    timer_divide: u32,              // 0x1A0 - Timer Divide Configuration
    _reserved11: [u32; 1],          // 0x1A4-0x1A7
}

/// Local APIC base address (default from x86_64 CPU)
///
/// NOTE: This should be discovered via ACPI MADT in production.
/// Using the standard x86 default address for now.
pub const LOCAL_APIC_DEFAULT_BASE: u64 = 0xFEE0_0000;

/// I/O APIC base address
///
/// NOTE: This should be discovered via ACPI MADT in production.
/// Using the standard x86 default address for now.
pub const IOAPIC_DEFAULT_BASE: u64 = 0xFEC0_0000;

/// Initialize the Local APIC
///
/// UEFI firmware typically initializes the Local APIC during boot.
/// This function ensures the LAPIC is enabled (bit 8 of SVR).
pub fn apic_local_init() {
    unsafe {
        let apic_base = LOCAL_APIC_DEFAULT_BASE;
        let svr_offset = 0x70; // Spurious Interrupt Vector Register

        let svr = (apic_base + svr_offset as u64) as *mut u32;

        // Enable Local APIC (set bit 8) and set spurious vector to 0xFF
        *svr = 0x100 | 0xFF;
    }
}

/// Send End of Interrupt (EOI) to the Local APIC
///
/// The IRQ number is not used by the Local APIC EOI register,
/// but we keep it for API compatibility.
pub fn apic_send_eoi(_irq: u32) {
    const LAPIC_EOI_OFFSET: u64 = 0x40;

    unsafe {
        let eoi_reg = (LOCAL_APIC_DEFAULT_BASE + LAPIC_EOI_OFFSET) as *mut u32;
        *eoi_reg = 0;
    }
}

/// Issue End of Interrupt (alias for apic_send_eoi)
pub fn apic_issue_eoi() {
    apic_send_eoi(0); // EOI number doesn't matter for LAPIC
}

/// Initialize I/O APIC for a specific IRQ
///
/// # Arguments
/// * `irq` - The IRQ number to configure (e.g., 1 for keyboard)
/// * `vector` - The interrupt vector to route to (e.g., 33 for IRQ1)
///
/// # Example
/// ```ignore
/// // Route IRQ1 (keyboard) to vector 33
/// apic_io_init(1, 33);
/// ```
pub fn apic_io_init(irq: u8, vector: u8) {
    const IOAPIC_BASE: u64 = 0xFEC0_0000;
    const IOAPIC_IOREGSEL: u64 = 0x00;
    const IOAPIC_IOWIN: u64 = 0x10;
    // Redirection table entries start at IOREGSEL 0x12
    // Each entry is 2 dwords (low + high)
    let irq_redir_offset: u32 = 0x12 + ((irq as u32 - 1) * 2);

    unsafe {
        let ioapic_sel = (IOAPIC_BASE + IOAPIC_IOREGSEL) as *mut u32;
        let ioapic_win = (IOAPIC_BASE + IOAPIC_IOWIN) as *mut u32;

        // Low dword: Vector in bits 0-7, delivery mode = 0 (fixed), mask = 0 (enabled)
        let low_dword = vector as u32;
        // High dword: Destination CPU 0 (BSP)
        let high_dword = 0;

        // Write low dword of redirection entry
        ioapic_sel.write_volatile(irq_redir_offset);
        ioapic_win.write_volatile(low_dword);
        // Write high dword of redirection entry
        ioapic_sel.write_volatile(irq_redir_offset + 1);
        ioapic_win.write_volatile(high_dword);
    }
}
