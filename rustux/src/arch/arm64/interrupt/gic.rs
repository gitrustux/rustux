// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ARM64 GIC (Generic Interrupt Controller)
//!
//! This module provides support for the ARM Generic Interrupt Controller (GIC),
//! which is the standard interrupt controller for ARM64 systems.
//!
//! # Supported GIC Versions
//!
//! - **GICv2**: Legacy GIC with up to 1020 interrupt IDs
//! - **GICv3**: Updated GIC with support for:
//!   - Message Signaled Interrupts (MSI)
//!   - Extended number of interrupt IDs
//!   - Interrupt virtualization
//!
//! # GIC Architecture
//!
//! The GIC consists of:
//! - **Distributor (GICD)**: Controls interrupt routing to CPUs
//! - **CPU Interfaces (GICC)**: Per-CPU interfaces for interrupt handling
//! - **Redistributors (GICR)**: GICv3 component for MPI support
//!
//! # Usage
//!
//! ```ignore
//! let gic = GicV2::new(gicd_base, gicc_base);
//! gic.init();
//! gic.enable_irq(32, 1)?;
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};
use alloc::boxed::Box;
use crate::arch::arm64::mm::PAddr;

/// ============================================================================
/// GIC Register Offsets
/// ============================================================================

/// GICv2 Distributor register offsets
pub mod gicd_offset {
    /// Control Register
    pub const CTLR: usize = 0x000;

    /// Interrupt Controller Type Register
    pub const TYPER: usize = 0x004;

    /// Interrupt Group Registers
    pub const IGROUPR: usize = 0x080;

    /// Interrupt Set-Enable Registers
    pub const ISENABLER: usize = 0x100;

    /// Interrupt Clear-Enable Registers
    pub const ICENABLER: usize = 0x180;

    /// Interrupt Set-Pending Registers
    pub const ISPENDR: usize = 0x200;

    /// Interrupt Clear-Pending Registers
    pub const ICPENDR: usize = 0x280;

    /// Software Generated Interrupt Register
    pub const SGIR: usize = 0xF00;

    /// Interrupt Configuration Registers
    pub const ICFGR: usize = 0xC00;
}

/// GICv2 CPU Interface register offsets
pub mod gicc_offset {
    /// Control Register
    pub const CTLR: usize = 0x000;

    /// Priority Mask Register
    pub const PMR: usize = 0x004;

    /// Binary Point Register
    pub const BPR: usize = 0x008;

    /// Interrupt Acknowledge Register
    pub const IAR: usize = 0x00C;

    /// End of Interrupt Register
    pub const EOIR: usize = 0x010;

    /// Running Priority Register
    pub const RPR: usize = 0x014;

    /// Highest Priority Pending Interrupt Register
    pub const HPPIR: usize = 0x018;

    /// End of Interrupt Register 1
    pub const EOIR1: usize = 0x0C0;
}

/// ============================================================================
/// GICv2 Controller
/// ============================================================================

/// GICv2 interrupt controller
///
/// Provides GICv2 support for ARM64 systems.
pub struct GicV2 {
    /// Distributor base address
    pub gicd_base: PAddr,

    /// CPU interface base address
    pub gicc_base: PAddr,

    /// Number of IRQ lines supported
    pub num_irq: AtomicUsize,

    /// Number of CPUs supported
    pub num_cpus: AtomicUsize,
}

impl GicV2 {
    /// Create a new GICv2 controller
    ///
    /// # Arguments
    ///
    /// * `gicd_base` - Physical address of GIC distributor
    /// * `gicc_base` - Physical address of GIC CPU interface
    pub const fn new(gicd_base: PAddr, gicc_base: PAddr) -> Self {
        Self {
            gicd_base,
            gicc_base,
            num_irq: AtomicUsize::new(0),
            num_cpus: AtomicUsize::new(0),
        }
    }

    /// Initialize the GIC
    ///
    /// This detects the GIC version and number of IRQ lines.
    pub fn init(&self) -> Result<(), &'static str> {
        // TODO: Implement GIC initialization
        // 1. Read TYPER to determine number of IRQs
        // 2. Configure interrupt groups
        // 3. Set priority mask
        // 4. Enable the GIC

        Err("GIC initialization not yet implemented")
    }

    /// Enable an IRQ line
    ///
    /// # Arguments
    ///
    /// * `irq` - IRQ number (SPI: 32-1019)
    /// * `cpu` - Target CPU (0-based)
    pub fn enable_irq(&self, irq: u32, cpu: u32) -> Result<(), &'static str> {
        if irq < 32 || irq >= 1020 {
            return Err("IRQ out of range");
        }

        // Calculate register and bit for SPI
        let reg_offset = gicd_offset::ISENABLER + ((irq as usize / 32) * 4);
        let bit = irq % 32;

        // TODO: Write to GICD_ISENABLER to enable the IRQ
        // TODO: Configure target CPU in GICD_ITARGETSR

        Ok(())
    }

    /// Disable an IRQ line
    pub fn disable_irq(&self, irq: u32) -> Result<(), &'static str> {
        if irq < 32 || irq >= 1020 {
            return Err("IRQ out of range");
        }

        // TODO: Write to GICD_ICENABLER to disable the IRQ

        Ok(())
    }

    /// Send End of Interrupt
    ///
    /// # Arguments
    ///
    /// * `irq` - IRQ number to acknowledge
    pub fn eoi(&self, irq: u32) {
        // TODO: Write IRQ to GICC_EOIR
    }

    /// Get the highest priority pending interrupt
    ///
    /// Returns the IRQ number or None if no interrupt is pending.
    pub fn get_pending(&self) -> Option<u32> {
        // TODO: Read GICC_IAR and return IRQ number
        None
    }

    /// Get number of IRQ lines
    pub fn num_irq(&self) -> usize {
        self.num_irq.load(Ordering::Relaxed)
    }

    /// Get number of CPUs
    pub fn num_cpus(&self) -> usize {
        self.num_cpus.load(Ordering::Relaxed)
    }
}

/// ============================================================================
/// GICv3 Controller (Placeholder)
/// ============================================================================

/// GICv3 interrupt controller
///
/// Provides GICv3 support with redistributors and extended IRQ support.
pub struct GicV3 {
    /// Distributor base address
    pub gicd_base: PAddr,

    /// Redistributor base address
    pub gicr_base: PAddr,

    /// CPU interface base address
    pub gicc_base: PAddr,

    /// Number of IRQ lines supported
    pub num_irq: AtomicUsize,
}

impl GicV3 {
    /// Create a new GICv3 controller
    ///
    /// # Arguments
    ///
    /// * `gicd_base` - Physical address of GIC distributor
    /// * `gicr_base` - Physical address of GIC redistributors
    /// * `gicc_base` - Physical address of GIC CPU interface
    pub const fn new(gicd_base: PAddr, gicr_base: PAddr, gicc_base: PAddr) -> Self {
        Self {
            gicd_base,
            gicr_base,
            gicc_base,
            num_irq: AtomicUsize::new(0),
        }
    }

    /// Initialize the GICv3
    pub fn init(&self) -> Result<(), &'static str> {
        Err("GICv3 initialization not yet implemented")
    }

    /// Enable an IRQ line
    pub fn enable_irq(&self, irq: u32, cpu: u32) -> Result<(), &'static str> {
        // TODO: Implement GICv3 IRQ enable
        Err("not implemented")
    }

    /// Disable an IRQ line
    pub fn disable_irq(&self, irq: u32) -> Result<(), &'static str> {
        // TODO: Implement GICv3 IRQ disable
        Err("not implemented")
    }

    /// Send End of Interrupt
    pub fn eoi(&self, irq: u32) {
        // TODO: Implement GICv3 EOI
    }
}

// ============================================================================
/// GIC Discovery (from ACPI)
/// ============================================================================

/// GIC version
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicVersion {
    /// GICv1
    V1 = 1,

    /// GICv2
    V2 = 2,

    /// GICv3
    V3 = 3,

    /// GICv4
    V4 = 4,
}

impl GicVersion {
    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::V1,
            2 => Self::V2,
            3 => Self::V3,
            4 => Self::V4,
            _ => Self::V2, // Default to v2
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self as u32
    }

    /// Get version as string
    pub const fn name(self) -> &'static str {
        match self {
            Self::V1 => "GICv1",
            Self::V2 => "GICv2",
            Self::V3 => "GICv3",
            Self::V4 => "GICv4",
        }
    }
}

/// GIC information from ACPI
#[repr(C)]
#[derive(Debug)]
pub struct GicInfo {
    /// GIC version
    pub version: GicVersion,

    /// Distributor physical address
    pub gicd_base: PAddr,

    /// Redistributor physical address (GICv3 only)
    pub gicr_base: Option<PAddr>,

    /// CPU interface physical address
    pub gicc_base: PAddr,
}

impl GicInfo {
    /// Create new GIC information
    pub fn new(version: GicVersion, gicd_base: PAddr, gicr_base: Option<PAddr>, gicc_base: PAddr) -> Self {
        Self {
            version,
            gicd_base,
            gicr_base,
            gicc_base,
        }
    }

    /// Get the appropriate GIC controller
    pub fn create_controller(&self) -> Result<Box<dyn crate::traits::InterruptController>, &'static str> {
        // TODO: Implement controller creation
        Err("GIC controller creation not yet implemented")
    }
}

// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gic_version() {
        assert_eq!(GicVersion::from_raw(0), GicVersion::V2);
        assert_eq!(GicVersion::from_raw(2), GicVersion::V2);
        assert_eq!(GicVersion::from_raw(3), GicVersion::V3);
        assert_eq!(GicVersion::V2.name(), "GICv2");
    }

    #[test]
    fn test_gic_v2_create() {
        let gic = GicV2::new(0x08000000, 0x08010000);
        assert_eq!(gic.gicd_base, 0x08000000);
        assert_eq!(gic.gicc_base, 0x08010000);
    }

    #[test]
    fn test_gic_irq_range() {
        let gic = GicV2::new(0, 0);
        assert!(gic.enable_irq(32, 0).is_ok()); // SPI
        assert!(gic.enable_irq(31, 0).is_err()); // SGI - invalid
        assert!(gic.enable_irq(1020, 0).is_err()); // Out of range
    }

    #[test]
    fn test_gic_v3_create() {
        let gic = GicV3::new(0x08000000, Some(0x080A0000), 0x08010000);
        assert_eq!(gic.gicd_base, 0x08000000);
        assert_eq!(gic.gicr_base, Some(0x080A0000));
        assert_eq!(gic.gicc_base, 0x08010000);
    }
}
