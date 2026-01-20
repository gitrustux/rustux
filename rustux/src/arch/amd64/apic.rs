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

/// Disable the legacy 8259A PIC
///
/// When using APIC mode, the legacy 8259A PIC must be disabled
/// by masking all IRQs. Otherwise, it will intercept interrupts
/// before they reach the IOAPIC.
///
/// The 8259A has two chips:
/// - Master PIC: Command port 0x20, Data port 0x21
/// - Slave PIC: Command port 0xA0, Data port 0xA1
///
/// To disable, we mask all IRQs by writing 0xFF to both data ports.
pub fn pic_disable() {
    const PIC1_CMD: u16 = 0x20;
    const PIC1_DATA: u16 = 0x21;
    const PIC2_CMD: u16 = 0xA0;
    const PIC2_DATA: u16 = 0xA1;

    unsafe {
        let msg = b"[PIC] Disabling 8259A PIC...\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Mask all IRQs on both PICs (write 0xFF to data ports)
        core::arch::asm!("out dx, al", in("dx") PIC1_DATA, in("al") 0xFFu8, options(nostack));
        core::arch::asm!("out dx, al", in("dx") PIC2_DATA, in("al") 0xFFu8, options(nostack));

        let msg = b"[PIC] All IRQs masked\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
    }
}

/// Initialize the Local APIC
///
/// UEFI firmware typically initializes the Local APIC during boot.
/// This function ensures the LAPIC is enabled (bit 8 of SVR).
pub fn apic_local_init() {
    // First, disable the legacy 8259A PIC
    // This must be done before using IOAPIC, otherwise the PIC
    // will intercept interrupts before they reach the IOAPIC
    pic_disable();

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

/// Probe the I/O APIC to verify it's accessible
///
/// Reads the IOAPIC ID and version registers to verify the IOAPIC
/// is responding at the expected base address.
fn ioapic_probe() {
    const IOAPIC_BASE: u64 = 0xFEC0_0000;
    const IOAPIC_IOREGSEL: u64 = 0x00;
    const IOAPIC_IOWIN: u64 = 0x10;
    const IOAPIC_ID_OFFSET: u32 = 0x00;
    const IOAPIC_VER_OFFSET: u32 = 0x01;

    unsafe {
        let ioapic_sel = (IOAPIC_BASE + IOAPIC_IOREGSEL) as *mut u32;
        let ioapic_win = (IOAPIC_BASE + IOAPIC_IOWIN) as *mut u32;

        // Read IOAPIC ID
        ioapic_sel.write_volatile(IOAPIC_ID_OFFSET);
        let id = ioapic_win.read_volatile();

        // Read IOAPIC Version
        ioapic_sel.write_volatile(IOAPIC_VER_OFFSET);
        let ver = ioapic_win.read_volatile();

        // Print IOAPIC info
        let msg = b"[IOAPIC] ID=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = (id >> 24) & 0x0F;  // IOAPIC ID is in bits 24-27
        let mut buf = [0u8; 8];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b" VER=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = ver & 0xFF;  // Version is in low 8 bits
        let mut buf = [0u8; 8];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b" MAX_REDIR=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = ((ver >> 16) & 0xFF) + 1;  // Max redirection entry
        let mut buf = [0u8; 8];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b"\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
    }
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
    // First, probe the IOAPIC to verify it's accessible
    ioapic_probe();
    const IOAPIC_BASE: u64 = 0xFEC0_0000;
    const IOAPIC_IOREGSEL: u64 = 0x00;
    const IOAPIC_IOWIN: u64 = 0x10;
    // Redirection table entries start at IOREGSEL 0x12
    // Each entry is 2 dwords (low + high)
    let irq_redir_offset: u32 = 0x12 + ((irq as u32 - 1) * 2);

    unsafe {
        // Debug: Print what we're about to write
        let msg = b"[IOAPIC] irq=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = irq;
        let mut buf = [0u8; 8];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b" vector=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = vector;
        let mut buf = [0u8; 8];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b"\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

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

        // Read back and verify
        ioapic_sel.write_volatile(irq_redir_offset);
        let read_low = ioapic_win.read_volatile();
        ioapic_sel.write_volatile(irq_redir_offset + 1);
        let read_high = ioapic_win.read_volatile();

        let msg = b"[IOAPIC] readback: low=0x";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = read_low;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            let digit = (n & 0xF) as u8;
            buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
            n >>= 4;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b" high=0x";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = read_high;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            let digit = (n & 0xF) as u8;
            buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
            n >>= 4;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b"\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        let msg = b"[IOAPIC] configured\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
    }
}
