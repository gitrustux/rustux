// Copyright 202 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Interrupt controller abstraction
//!
//! This module provides a trait for interrupt controller operations across
//! different architectures (APIC, GIC, PLIC, PIC).

use crate::kernel::arch::amd64::apic;

/// Interrupt controller trait - architecture-independent interface
pub trait InterruptController {
    /// Enable an interrupt
    fn enable_irq(&mut self, irq: u8, vector: u8);

    /// Disable an interrupt
    fn disable_irq(&mut self, irq: u8);

    /// Send end-of-interrupt signal
    fn send_eoi(&self, irq: u8);

    /// Initialize the interrupt controller
    fn init(&mut self) -> Result<(), &'static str>;
}

/// x86_64 Interrupt Controller (Local APIC + IOAPIC)
pub struct X86_64InterruptController {
    pub enabled: bool,
}

impl X86_64InterruptController {
    pub fn new() -> Self {
        Self {
            enabled: false,
        }
    }
}

impl InterruptController for X86_64InterruptController {
    /// Enable IRQ in IOAPIC redirection table
    fn enable_irq(&mut self, irq: u8, vector: u8) {
        apic::apic_io_init(irq, vector);
    }

    /// Disable an interrupt
    fn disable_irq(&mut self, irq: u8) {
        // TODO: Set mask bit 16 in IOAPIC redirection entry
        // This requires reading current value, OR-ing in the mask bit, writing back
    }

    /// Send end-of-interrupt signal
    fn send_eoi(&self, _irq: u8) {
        apic:: apic_send_eoi(0); // IRQ number doesn't matter for LAPIC
    }

    /// Initialize the interrupt controller
    fn init(&mut self) -> Result<(), &'static str> {
        // Initialize Local APIC (UEFI already did this, but ensure it's enabled)
        apic::apic_local_init();

        // Initialize IOAPIC for IRQ1 (keyboard)
        self.enable_irq(1, 33)?;

        self.enabled = true;
        Ok(())
    }
}
