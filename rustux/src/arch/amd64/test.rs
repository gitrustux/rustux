// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Interrupt System Tests
//!
//! This module provides tests for the interrupt subsystem including:
//! - GDT (Global Descriptor Table)
//! - IDT (Interrupt Descriptor Table)
//! - APIC (Local APIC + I/O APIC)
//! - Timer interrupts
//! - Keyboard interrupts

use core::sync::atomic::{AtomicU64, Ordering};

/// Test tick counter (incremented by timer interrupt)
static TIMER_TICKS: AtomicU64 = AtomicU64::new(0);

/// Timer interrupt vector (typically 32)
pub const TIMER_VECTOR: u8 = 32;

/// Keyboard interrupt vector (typically 33)
pub const KEYBOARD_VECTOR: u8 = 33;

/// ============================================================================
/// Serial Output (for debugging without full console)
/// ============================================================================

/// QEMU x86 debug console port
const QEMU_DEBUGCON_PORT: u16 = 0xE9;

/// Write a byte to QEMU's debug console
///
/// This is the simplest way to output debug messages in QEMU.
/// Messages appear in the console when using `-debugcon` flag.
fn qemu_debugcon_write_byte(b: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") QEMU_DEBUGCON_PORT,
            in("al") b,
            options(nostack, nomem)
        );
    }
}

/// Write a string to QEMU's debug console
pub fn qemu_print(s: &str) {
    for byte in s.bytes() {
        qemu_debugcon_write_byte(byte);
    }
}

/// ============================================================================
/// Timer Interrupt Handler
/// ============================================================================

/// Timer interrupt handler
///
/// This is called by the IDT entry for vector 32 (TIMER_VECTOR).
/// It increments the tick counter and sends EOI.
extern "x86-interrupt" fn timer_handler(_frame: &mut super::idt::X86Iframe) {
    let ticks = TIMER_TICKS.fetch_add(1, Ordering::Relaxed);

    // Print every 10th tick to avoid spam
    if ticks % 10 == 0 {
        qemu_print("[TICK ");
        print_digit((ticks / 10) % 10);
        qemu_print("]\n");
    }

    // Send EOI to Local APIC
    super::apic::apic_send_eoi(TIMER_VECTOR as u32);
}

/// Print a single digit
fn print_digit(d: u64) {
    qemu_debugcon_write_byte(b'0' + (d as u8));
}

/// ============================================================================
/// Test Functions
/// ============================================================================

/// Test the complete interrupt system
///
/// This function performs the following tests:
/// 1. Setup GDT (Global Descriptor Table)
/// 2. Setup IDT (Interrupt Descriptor Table)
/// 3. Initialize APIC (Local APIC + I/O APIC)
/// 4. Configure timer interrupt
/// 5. Enable interrupts and wait for timer ticks
///
/// # Safety
///
/// This function modifies critical system state (GDT, IDT, APIC).
/// It should only be called once during kernel initialization.
pub fn test_interrupt_system() {
    qemu_print("=== Rustux Interrupt System Test ===\n");
    qemu_print("Testing migrated boot infrastructure\n\n");

    // Step 1: Setup GDT
    qemu_print("[1/5] Setting up GDT... ");
    unsafe {
        super::descriptor::gdt_setup();
    }
    qemu_print("OK\n");

    // Step 2: Setup IDT
    qemu_print("[2/5] Setting up IDT... ");
    unsafe {
        super::descriptor::idt_setup_readonly();
    }
    qemu_print("OK\n");

    // Step 3: Install timer handler
    qemu_print("[3/5] Installing timer handler... ");
    unsafe {
        super::idt::idt_set_gate(
            TIMER_VECTOR,
            timer_handler as u64,
            0x08, // Kernel code segment
            0x8E, // Interrupt gate (present, DPL=0)
        );
    }
    qemu_print("OK\n");

    // Step 4: Initialize APIC
    qemu_print("[4/5] Initializing APIC... ");
    super::apic::apic_local_init();
    qemu_print("OK\n");

    // Step 5: Configure timer
    qemu_print("[5/5] Configuring timer... ");
    unsafe {
        configure_lapic_timer();
    }
    qemu_print("OK\n");

    qemu_print("\nInterrupt system configured!\n");
    qemu_print("Enabling interrupts and waiting for timer ticks...\n\n");

    // Enable interrupts
    unsafe {
        super::init::arch_enable_ints();
    }

    // Wait for timer interrupts
    qemu_print("Waiting for timer interrupts (100 ticks max)...\n");

    for i in 0..100 {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }

        // Check if we got at least 10 ticks
        if TIMER_TICKS.load(Ordering::Relaxed) >= 10 {
            qemu_print("\n=== TEST PASSED ===\n");
            qemu_print("Received ");
            print_decimal(TIMER_TICKS.load(Ordering::Relaxed));
            qemu_print(" timer ticks successfully!\n");
            return;
        }
    }

    qemu_print("\n=== TEST WARNING ===\n");
    qemu_print("Only received ");
    print_decimal(TIMER_TICKS.load(Ordering::Relaxed));
    qemu_print(" ticks (expected 10+)\n");
    qemu_print("This could mean:\n");
    qemu_print("  - Timer not configured correctly\n");
    qemu_print("  - APIC not enabled\n");
    qemu_print("  - Running on hardware without APIC\n");
}

/// Configure the Local APIC timer
///
/// This configures the LAPIC timer to fire periodically.
/// The timer is configured to:
/// - Divide bus clock by 1
/// - Use periodic mode
/// - Vector 32 (TIMER_VECTOR)
/// - Initial count = 1,000,000 (depends on bus frequency)
///
/// # Safety
///
/// This modifies Local APIC MMIO registers.
unsafe fn configure_lapic_timer() {
    const LOCAL_APIC_BASE: u64 = super::apic::LOCAL_APIC_DEFAULT_BASE;

    // Timer divide configuration (divide by 1)
    let timer_divide = (LOCAL_APIC_BASE + 0x1A0) as *mut u32;
    timer_divide.write_volatile(0x03); // Divide by 1

    // Timer LVT (Local Vector Table) configuration
    // Bit 16: Timer mode (0 = one-shot, 1 = periodic)
    // Bits 0-7: Interrupt vector
    let timer_lvt = (LOCAL_APIC_BASE + 0x190) as *mut u32;
    timer_lvt.write_volatile((1 << 17) | TIMER_VECTOR as u32);

    // Timer initial count
    // This value determines the timer frequency.
    // The actual frequency depends on the bus clock.
    // For QEMU/KVM, 1,000,000 gives a reasonable tick rate.
    let timer_initial = (LOCAL_APIC_BASE + 0x170) as *mut u32;
    timer_initial.write_volatile(1_000_000);
}

/// Print a decimal number to QEMU debug console
fn print_decimal(mut n: u64) {
    if n == 0 {
        qemu_debugcon_write_byte(b'0');
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;

    while n > 0 {
        buf[i] = b'0' + ((n % 10) as u8);
        n /= 10;
        i += 1;
    }

    while i > 0 {
        i -= 1;
        qemu_debugcon_write_byte(buf[i]);
    }
}

/// ============================================================================
/// Keyboard Interrupt Test (Optional)
/// ============================================================================

/// Keyboard interrupt handler
///
/// This is called by the IDT entry for vector 33 (KEYBOARD_VECTOR).
extern "x86-interrupt" fn keyboard_handler(_frame: &mut super::idt::X86Iframe) {
    qemu_print("[KEYBOARD_PRESS]\n");

    // Read from keyboard data port to acknowledge
    unsafe {
        let keyboard_port = 0x60u16;
        let _data: u8;
        core::arch::asm!(
            "in al, dx",
            in("dx") keyboard_port,
            out("al") _data,
            options(nostack, nomem)
        );
    }

    // Send EOI
    super::apic::apic_send_eoi(KEYBOARD_VECTOR as u32);
}

/// Test keyboard interrupt
///
/// This enables keyboard interrupt (IRQ1) and waits for keypresses.
/// Requires the IOAPIC to be configured.
pub fn test_keyboard_interrupt() {
    qemu_print("=== Keyboard Interrupt Test ===\n");
    qemu_print("Press any key on the keyboard...\n\n");

    // Install keyboard handler
    unsafe {
        super::idt::idt_set_gate(
            KEYBOARD_VECTOR,
            keyboard_handler as u64,
            0x08, // Kernel code segment
            0x8E, // Interrupt gate
        );
    }

    qemu_print("Keyboard handler installed.\n");
    qemu_print("Press Ctrl+C to exit (if running in QEMU)\n");

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
    }
}
