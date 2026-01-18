// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V PLIC (Platform-Level Interrupt Controller)
//!
//! This module provides support for the RISC-V Platform-Level Interrupt Controller,
//! which handles external interrupts for RISC-V systems.
//!
//! # PLIC Architecture
//!
//! The PLIC consists of:
//! - **Interrupt Sources**: Devices that can generate interrupts
//! - **Priority Registers**: Per-interrupt priority values
//! - **Pending Registers**: Per-interrupt pending bits
//! - **Enable Registers**: Per-hart enable bits for each interrupt
//! - **Threshold/Claim Registers**: Per-hart interface for interrupt handling
//!
//! # Interrupt Priority Flow
//!
//! 1. Device asserts interrupt line
//! 2. PLIC sets pending bit for interrupt source
//! 3. If interrupt is enabled for hart and priority > threshold:
//!    - Hart can claim the interrupt
//!    - PLIC returns highest priority pending interrupt ID
//! 4. Hart handles interrupt and completes it
//! 5. PLIC allows next interrupt of same source to be pending

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// ============================================================================
/// PLIC Register Offsets
/// ============================================================================

/// PLIC priority register offsets
pub mod plic_offset {
    /// Priority registers (4 bytes per interrupt)
    pub const PRIORITY_BASE: usize = 0x0000;

    /// Pending registers (1 bit per interrupt)
    pub const PENDING_BASE: usize = 0x1000;

    /// Enable registers (1 bit per interrupt, per hart)
    pub const ENABLE_BASE: usize = 0x2000;

    /// Threshold/claim registers (per hart context)
    pub const CONTEXT_BASE: usize = 0x200000;

    /// Claim/complete register offset within context
    pub const CLAIM_COMPLETE: usize = 0x04;

    /// Threshold register offset within context
    pub const THRESHOLD: usize = 0x00;
}

/// ============================================================================
/// PLIC Configuration
/// ============================================================================

/// Maximum number of interrupt sources (excluding 0 which is "no interrupt")
pub const PLIC_MAX_SOURCES: usize = 1024;

/// Maximum number of harts supported
pub const PLIC_MAX_HARTS: usize = 8;

/// Maximum context per hart (typically M-mode and S-mode)
pub const PLIC_MAX_CONTEXT_PER_HART: usize = 2;

/// Priority value range (0-7, 7 = highest)
pub const PLIC_MAX_PRIORITY: u32 = 7;

/// ============================================================================
/// PLIC IRQ Number
/// ============================================================================

/// PLIC IRQ number
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlicIrq(pub u32);

impl PlicIrq {
    /// Invalid IRQ (no interrupt pending)
    pub const NONE: Self = Self(0);

    /// Create a new IRQ number
    pub const fn new(irq: u32) -> Self {
        Self(irq)
    }

    /// Get the raw IRQ number
    pub const fn into_inner(self) -> u32 {
        self.0
    }

    /// Check if IRQ is valid
    pub const fn is_valid(self) -> bool {
        self.0 > 0 && self.0 < PLIC_MAX_SOURCES as u32
    }
}

/// ============================================================================
/// PLIC Priority
/// ============================================================================

/// PLIC priority value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlicPriority(pub u32);

impl PlicPriority {
    /// Minimum priority (interrupts never delivered)
    pub const MIN: Self = Self(0);

    /// Maximum priority (7 = highest)
    pub const MAX: Self = Self(7);

    /// Default priority
    pub const DEFAULT: Self = Self(1);

    /// Create a new priority value
    pub const fn new(priority: u32) -> Self {
        // Manual min instead of .min() for const compatibility
        let clamped = if priority > PLIC_MAX_PRIORITY {
            PLIC_MAX_PRIORITY
        } else {
            priority
        };
        Self(clamped)
    }

    /// Get the raw priority value
    pub const fn into_inner(self) -> u32 {
        self.0
    }
}

/// ============================================================================
/// PLIC Hart Context
/// ============================================================================

/// PLIC hart context (M-mode or S-mode)
#[repr(C)]
#[derive(Debug)]
pub struct PlicHartContext {
    /// Hart ID
    pub hart_id: usize,

    /// Context ID (0 for M-mode, 1 for S-mode typically)
    pub context_id: usize,

    /// Base address of this context's registers
    pub base: usize,

    /// Priority threshold
    pub threshold: AtomicU32,
}

impl PlicHartContext {
    /// Create a new PLIC hart context
    ///
    /// # Arguments
    ///
    /// * `hart_id` - Hart ID
    /// * `context_id` - Context ID (0 = M-mode, 1 = S-mode)
    /// * `plic_base` - Base address of PLIC registers
    pub fn new(hart_id: usize, context_id: usize, plic_base: usize) -> Self {
        // Calculate context base address
        let context_offset = plic_offset::CONTEXT_BASE +
            (hart_id * PLIC_MAX_CONTEXT_PER_HART + context_id) * 0x1000;
        let base = plic_base + context_offset;

        Self {
            hart_id,
            context_id,
            base,
            threshold: AtomicU32::new(0),
        }
    }

    /// Get the threshold register address
    pub fn threshold_addr(&self) -> usize {
        self.base + plic_offset::THRESHOLD
    }

    /// Get the claim/complete register address
    pub fn claim_complete_addr(&self) -> usize {
        self.base + plic_offset::CLAIM_COMPLETE
    }

    /// Set the priority threshold
    ///
    /// Interrupts with priority <= threshold will not be delivered.
    pub fn set_threshold(&self, threshold: PlicPriority) {
        self.threshold.store(threshold.into_inner(), Ordering::Release);
        // TODO: Write to threshold register
    }

    /// Get the current threshold
    pub fn get_threshold(&self) -> PlicPriority {
        PlicPriority(self.threshold.load(Ordering::Acquire))
    }

    /// Claim the highest priority pending interrupt
    ///
    /// Returns the IRQ number or `PlicIrq::NONE` if no interrupt is pending.
    pub fn claim(&self) -> PlicIrq {
        // TODO: Read claim register and return IRQ number
        PlicIrq::NONE
    }

    /// Complete handling of an interrupt
    ///
    /// # Arguments
    ///
    /// * `irq` - IRQ number to complete
    pub fn complete(&self, irq: PlicIrq) {
        // TODO: Write IRQ to complete register
        let _ = irq;
    }
}

/// ============================================================================
/// PLIC Controller
/// ============================================================================

/// PLIC (Platform-Level Interrupt Controller)
///
/// Manages external interrupts for RISC-V systems.
pub struct Plic {
    /// Base address of PLIC registers
    pub base: usize,

    /// Number of interrupt sources
    pub num_sources: AtomicUsize,

    /// Number of harts
    pub num_harts: AtomicUsize,

    /// Hart contexts
    pub contexts: [Option<PlicHartContext>; PLIC_MAX_HARTS * PLIC_MAX_CONTEXT_PER_HART],
}

impl Plic {
    /// Create a new PLIC instance
    ///
    /// # Arguments
    ///
    /// * `base` - Base address of PLIC registers
    pub const fn new(base: usize) -> Self {
        const INIT_CONTEXT: Option<PlicHartContext> = None;
        Self {
            base,
            num_sources: AtomicUsize::new(0),
            num_harts: AtomicUsize::new(0),
            contexts: [INIT_CONTEXT; PLIC_MAX_HARTS * PLIC_MAX_CONTEXT_PER_HART],
        }
    }

    /// Initialize the PLIC
    ///
    /// This detects the number of interrupt sources and harts.
    pub fn init(&self) -> Result<(), &'static str> {
        // TODO: Implement PLIC initialization
        // 1. Detect number of interrupt sources
        // 2. Detect number of harts
        // 3. Disable all interrupts
        // 4. Set all priorities to 0
        // 5. Set all thresholds to 0 (allow all interrupts)

        Err("PLIC initialization not yet implemented")
    }

    /// Get the priority register address for an interrupt
    pub fn priority_addr(&self, irq: PlicIrq) -> usize {
        self.base + plic_offset::PRIORITY_BASE + (irq.into_inner() as usize * 4)
    }

    /// Set the priority for an interrupt
    ///
    /// # Arguments
    ///
    /// * `irq` - IRQ number
    /// * `priority` - Priority value (0-7)
    pub fn set_priority(&self, irq: PlicIrq, priority: PlicPriority) {
        // TODO: Write to priority register
        let _ = (irq, priority);
    }

    /// Get the priority for an interrupt
    pub fn get_priority(&self, irq: PlicIrq) -> PlicPriority {
        // TODO: Read from priority register
        let _ = irq;
        PlicPriority::DEFAULT
    }

    /// Get the pending register address for an interrupt
    pub fn pending_addr(&self) -> usize {
        self.base + plic_offset::PENDING_BASE
    }

    /// Check if an interrupt is pending
    pub fn is_pending(&self, irq: PlicIrq) -> bool {
        // TODO: Read pending bit
        let _ = irq;
        false
    }

    /// Get the enable register address for a hart and interrupt
    ///
    /// # Arguments
    ///
    /// * `hart_id` - Hart ID
    /// * `context_id` - Context ID (0 = M-mode, 1 = S-mode)
    /// * `irq` - IRQ number
    pub fn enable_addr(&self, hart_id: usize, context_id: usize, irq: PlicIrq) -> usize {
        let enable_offset = (hart_id * PLIC_MAX_CONTEXT_PER_HART + context_id) * 0x80;
        let reg_offset = (irq.into_inner() as usize / 32) * 4;
        self.base + plic_offset::ENABLE_BASE + enable_offset + reg_offset
    }

    /// Enable an interrupt for a hart
    ///
    /// # Arguments
    ///
    /// * `hart_id` - Hart ID
    /// * `context_id` - Context ID (0 = M-mode, 1 = S-mode)
    /// * `irq` - IRQ number
    pub fn enable_irq(&self, hart_id: usize, context_id: usize, irq: PlicIrq) -> Result<(), &'static str> {
        if !irq.is_valid() {
            return Err("IRQ out of range");
        }

        // TODO: Set enable bit
        let _ = (hart_id, context_id);
        Ok(())
    }

    /// Disable an interrupt for a hart
    pub fn disable_irq(&self, hart_id: usize, context_id: usize, irq: PlicIrq) -> Result<(), &'static str> {
        if !irq.is_valid() {
            return Err("IRQ out of range");
        }

        // TODO: Clear enable bit
        let _ = (hart_id, context_id);
        Ok(())
    }

    /// Get or create a hart context
    pub fn get_context(&self, hart_id: usize, context_id: usize) -> Option<&PlicHartContext> {
        let idx = hart_id * PLIC_MAX_CONTEXT_PER_HART + context_id;
        if idx >= self.contexts.len() {
            return None;
        }
        self.contexts[idx].as_ref()
    }

    /// Add a hart context
    pub fn add_context(&mut self, hart_id: usize, context_id: usize) -> Result<(), &'static str> {
        let idx = hart_id * PLIC_MAX_CONTEXT_PER_HART + context_id;
        if idx >= self.contexts.len() {
            return Err("Context index out of range");
        }

        self.contexts[idx] = Some(PlicHartContext::new(hart_id, context_id, self.base));
        Ok(())
    }

    /// Get the number of interrupt sources
    pub fn num_sources(&self) -> usize {
        self.num_sources.load(Ordering::Relaxed)
    }

    /// Get the number of harts
    pub fn num_harts(&self) -> usize {
        self.num_harts.load(Ordering::Relaxed)
    }
}

/// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plic_irq() {
        let irq = PlicIrq::new(32);
        assert_eq!(irq.into_inner(), 32);
        assert!(irq.is_valid());

        assert!(!PlicIrq::NONE.is_valid());
        assert!(!PlicIrq::new(0).is_valid());
        assert!(!PlicIrq::new(PLIC_MAX_SOURCES as u32).is_valid());
    }

    #[test]
    fn test_plic_priority() {
        let prio = PlicPriority::new(5);
        assert_eq!(prio.into_inner(), 5);

        assert_eq!(PlicPriority::MAX.into_inner(), 7);
        assert_eq!(PlicPriority::MIN.into_inner(), 0);

        // Test clamping
        assert_eq!(PlicPriority::new(10).into_inner(), 7);
    }

    #[test]
    fn test_plic_create() {
        let plic = Plic::new(0x0C000000);
        assert_eq!(plic.base, 0x0C000000);
    }

    #[test]
    fn test_plic_context() {
        let plic = Plic::new(0x0C000000);
        let context = PlicHartContext::new(0, 1, plic.base);

        assert_eq!(context.hart_id, 0);
        assert_eq!(context.context_id, 1);
    }

    #[test]
    fn test_enable_irq_invalid() {
        let plic = Plic::new(0);
        assert!(plic.enable_irq(0, 0, PlicIrq::NONE).is_err());
        assert!(plic.enable_irq(0, 0, PlicIrq::new(PLIC_MAX_SOURCES as u32)).is_err());
    }

    #[test]
    fn test_plic_add_context() {
        let mut plic = Plic::new(0);
        assert!(plic.add_context(0, 0).is_ok());
        assert!(plic.add_context(0, 1).is_ok());

        assert!(plic.get_context(0, 0).is_some());
        assert!(plic.get_context(0, 1).is_some());
        assert!(plic.get_context(1, 0).is_none());
    }
}
