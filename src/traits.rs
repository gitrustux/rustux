// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Cross-architecture interrupt traits
//!
//! This module defines common interrupt-related traits used across all architectures.
//!
//! Each architecture (amd64, arm64, riscv64) implements these traits.

/// Trait for interrupt controller operations
///
/// This trait provides a unified interface for different interrupt controllers:
/// - x86_64: APIC/x2APIC (Local APIC + I/O APIC)
/// - ARM64: GIC (Generic Interrupt Controller)
/// - RISC-V: PLIC (Platform-Level Interrupt Controller)
pub trait InterruptController {
    /// Enable an interrupt
    ///
    /// # Arguments
    /// * `irq` - The IRQ number to enable
    /// * `vector` - The interrupt vector to route to
    fn enable_irq(&mut self, irq: u64, vector: u64);

    /// Disable an interrupt
    ///
    /// # Arguments
    /// * `irq` - The IRQ number to disable
    fn disable_irq(&mut self, irq: u64);

    /// Send end-of-interrupt signal
    ///
    /// # Arguments
    /// * `irq` - The IRQ number to send EOI for (may be unused by some controllers)
    fn send_eoi(&self, irq: u64);

    /// Initialize the interrupt controller
    ///
    /// # Returns
    /// * `Ok(())` if initialization succeeded
    /// * `Err(&'static str)` if initialization failed with an error message
    fn init(&mut self) -> Result<(), &'static str>;
}

/// Interrupt trigger modes
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptTriggerMode {
    /// Edge-triggered interrupt
    Edge = 0,
    /// Level-triggered interrupt
    Level = 1,
}

/// Interrupt polarity
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPolarity {
    /// Active high
    ActiveHigh = 0,
    /// Active low
    ActiveLow = 1,
}

/// Interrupt delivery modes
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum InterruptDeliveryMode {
    /// Fixed delivery
    Fixed = 0,
    /// Lowest priority delivery
    Lowest = 1,
    /// SMI (System Management Interrupt)
    Smi = 2,
    /// NMI (Non-Maskable Interrupt)
    Nmi = 4,
    /// INIT (Initialization request)
    Init = 5,
    /// Startup IPI (Inter-Processor Interrupt)
    Startup = 6,
    /// External interrupt
    ExtInt = 7,
}
