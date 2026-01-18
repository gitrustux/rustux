// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 interrupt controller implementation (Local APIC + IOAPIC)
//!
//! This module provides the interrupt controller implementation for x86_64,
//! implementing the cross-architecture InterruptController trait.

use crate::traits::InterruptController;
use super::apic;

/// x86_64 Interrupt Controller (Local APIC + IOAPIC)
///
/// This struct implements the InterruptController trait for x86_64,
/// using the Local APIC for EOI and the I/O APIC for IRQ routing.
pub struct X86_64InterruptController {
    /// Whether the interrupt controller has been initialized
    pub enabled: bool,
}

impl X86_64InterruptController {
    /// Create a new x86_64 interrupt controller
    pub fn new() -> Self {
        Self {
            enabled: false,
        }
    }
}

impl Default for X86_64InterruptController {
    fn default() -> Self {
        Self::new()
    }
}

impl InterruptController for X86_64InterruptController {
    /// Enable IRQ in IOAPIC redirection table
    ///
    /// # Arguments
    /// * `irq` - The IRQ number to enable (e.g., 1 for keyboard)
    /// * `vector` - The interrupt vector to route to (e.g., 33 for IRQ1)
    ///
    /// # Example
    /// ```ignore
    /// let mut controller = X86_64InterruptController::new();
    /// controller.enable_irq(1, 33); // Route IRQ1 to vector 33
    /// ```
    fn enable_irq(&mut self, irq: u64, vector: u64) {
        apic::apic_io_init(irq as u8, vector as u8);
    }

    /// Disable an interrupt
    ///
    /// TODO: Set mask bit 16 in IOAPIC redirection entry.
    /// This requires reading current value, OR-ing in the mask bit, writing back.
    fn disable_irq(&mut self, _irq: u64) {
        // TODO: Implement by setting mask bit in IOAPIC redirection entry
    }

    /// Send end-of-interrupt signal to the Local APIC
    ///
    /// The IRQ number is not used by the Local APIC EOI register,
    /// but we keep it for API compatibility.
    fn send_eoi(&self, _irq: u64) {
        apic::apic_send_eoi(0); // IRQ number doesn't matter for LAPIC
    }

    /// Initialize the interrupt controller
    ///
    /// This initializes the Local APIC (which UEFI typically already did)
    /// and configures the I/O APIC for interrupt routing.
    ///
    /// # Returns
    /// * `Ok(())` if initialization succeeded
    /// * `Err(&'static str)` if initialization failed
    fn init(&mut self) -> Result<(), &'static str> {
        // Initialize Local APIC (UEFI already did this, but ensure it's enabled)
        apic::apic_local_init();

        // Initialize IOAPIC for IRQ1 (keyboard) - route to vector 33
        self.enable_irq(1, 33);

        self.enabled = true;
        Ok(())
    }
}
