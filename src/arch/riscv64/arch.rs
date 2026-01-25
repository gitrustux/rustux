// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V Architecture Support
//!
//! This module provides RISC-V architecture support for the Rustux kernel.
//! Currently a placeholder for future implementation.
//!
//! # Status
//!
//! ⚠️ **Placeholder Implementation**
//!
//! The RISC-V support is planned but not yet implemented. This module provides:
//! - Basic type definitions for RISC-V compatibility
//! - Placeholder interrupt controller stub
//! - Architecture trait implementations (to be completed)
//!
//! # Planned Features
//!
//! - **PLIC (Platform-Level Interrupt Controller)**: Standard RISC-V interrupt controller
//! - **MMU**: Sv39 and Sv48 page table management
//! - **SMP**: Hart-based multiprocessing with SBI
//! - **Exception handling**: Trap handling
//! - **Clint**: Core-local interruptor

use core::sync::atomic::{AtomicUsize, Ordering};

/// Maximum number of harts for RISC-V
pub const RISCV_MAX_HARTS: usize = 8;

/// RISC-V page size (4KB)
pub const RISCV_PAGE_SIZE: usize = 4096;

/// RISC-V page shift
pub const RISCV_PAGE_SHIFT: usize = 12;

// ============================================================================
/// RISC-V Interrupt Controller (Placeholder)
// ============================================================================

/// RISC-V interrupt controller placeholder
///
/// This will be implemented with PLIC (Platform-Level Interrupt Controller) support.
pub struct RiscvInterruptController {
    /// Number of IRQ lines
    max_irq: AtomicUsize,
}

impl RiscvInterruptController {
    /// Create a new RISC-V interrupt controller
    pub const fn new() -> Self {
        Self {
            max_irq: AtomicUsize::new(1024),
        }
    }

    /// Get the maximum number of IRQ lines
    pub fn max_irq(&self) -> usize {
        self.max_irq.load(Ordering::Relaxed)
    }
}

// ============================================================================
/// RISC-V Hart (CPU)
// ============================================================================

/// Hart (hardware thread) information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HartInfo {
    /// Hart ID
    pub hart_id: usize,

    /// hart ID from mhartid register
    pub mhartid: usize,

    /// Is bootstrap hart
    pub is_bootstrap: bool,
}

impl HartInfo {
    /// Create new hart info
    pub const fn new(hart_id: usize, mhartid: usize, is_bootstrap: bool) -> Self {
        Self {
            hart_id,
            mhartid,
            is_bootstrap,
        }
    }
}

/// Hart information array
static mut HART_INFO: [HartInfo; RISCV_MAX_HARTS] = [HartInfo::new(0, 0, false); RISCV_MAX_HARTS];

/// Get hart information
///
/// # Safety
///
/// The hart_id must be valid.
pub unsafe fn get_hart_info(hart_id: usize) -> &'static mut HartInfo {
    &mut HART_INFO[hart_id]
}

/// Set hart information
///
/// # Safety
///
/// The hart_id must be valid.
pub unsafe fn set_hart_info(hart_id: usize, info: HartInfo) {
    HART_INFO[hart_id] = info;
}

/// Get bootstrap hart ID
pub fn get_bootstrap_hart() -> usize {
    unsafe {
        for i in 0..RISCV_MAX_HARTS {
            if HART_INFO[i].is_bootstrap {
                return HART_INFO[i].hart_id;
            }
        }
    }
    0 // Default to hart 0
}

// ============================================================================
/// RISC-V Supervisor Binary Interface (SBI)
// ============================================================================

/// SBI extension IDs
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiExtension {
    /// Base extension
    Base = 0x10,
    /// Timer extension
    Timer = 0x54494D4F,
    /// IPI extension
    Ipi = 0x7350494E,
    /// RFENCE extension
    Rfence = 0x52464E45,
    /// Hart state management extension
    HartState = 0x48534D4E,
}

/// SBI function IDs for Base extension
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiFunction {
    /// Get SBI version
    GetSbiVersion = 0x00,

    /// Get SBI implementation ID
    GetSbiImplId = 0x01,

    /// Get SBI implementation version
    GetSbiImplVersion = 0x02,

    /// Probe extension
    ProbeExtension = 0x03,

    /// Get machine vendor ID
    GetMvendorId = 0x04,

    /// Get machine architecture ID
    GetMarchId = 0x05,

    /// Get machine implementation ID
    GetMimpId = 0x06,

    /// Get machine name
    GetMname = 0x08,

    /// Get extension version
    GetExtensionVersion = 0x09,

    /// Get Mtime
    GetMtime = 0x10,

    /// Set timer
    SetTimer = 0x11,
}

impl SbiFunction {
    /// Get function ID
    pub const fn id(&self) -> u64 {
        *self as u64
    }
}

/// SBI call return type
pub type SbiCall = (SbiRet, u64);

/// SBI return values
#[repr(i64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiRet {
    /// Success
    Success = 0,

    /// Failed
    Failed = -1,

    /// Not supported
    NotSupported = -2,

    /// Invalid address
    InvalidAddress = -3,

    /// Already available
    AlreadyAvailable = -4,
}

impl SbiRet {
    /// Create from raw value
    pub const fn from_raw(raw: i64) -> Self {
        match raw {
            0 => Self::Success,
            -1 => Self::Failed,
            -2 => Self::NotSupported,
            -3 => Self::InvalidAddress,
            -4 => Self::AlreadyAvailable,
            _ => Self::Failed,
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> i64 {
        self as i64
    }
}

/// Make an SBI call
///
/// # Arguments
///
/// * `extension` - SBI extension ID
/// * `function` - SBI function ID
/// * `args` - Arguments to pass
///
/// # Safety
///
/// Must be called from RISC-V code with proper arguments.
pub unsafe fn sbi_call(extension: SbiExtension, function: SbiFunction, args: [u64; 6]) -> (SbiRet, u64) {
    // TODO: Implement actual SBI call using ecall instruction
    // The assembly would be:
    // ecall
    // Where a7 = extension, a6 = function, a0-a5 = args

    // For now, return not supported
    (SbiRet::NotSupported, 0)
}

/// Get SBI version
pub fn get_sbi_version() -> (u64, u64) {
    unsafe {
        let (ret, value) = sbi_call(
            SbiExtension::Base,
            SbiFunction::GetSbiVersion,
            [0; 6],
        );
        (ret.into_raw() as u64, value)
    }
}

// ============================================================================
/// RISC-V CLINT (Core-Local Interruptor)
/// ============================================================================

/// CLINT (Core-Local Interruptor) registers
#[repr(C)]
pub struct Clint {
    /// mtime register (memory-mapped)
    pub mtime: *mut u64,

    /// mtimecmp register (per-hart)
    pub mtimecmp: *mut u64,

    /// msip register (per-hart)
    pub msip: *mut u32,
}

impl Clint {
    /// Create CLINT from base address
    ///
    /// # Arguments
    ///
    /// * `base` - Base address of CLINT region
    pub fn from_base(base: usize) -> Self {
        let mtime = base as *mut u64;
        let mtimecmp = (base + 0x4000) as *mut u64; // Offset for first hart
        let msip = (base + 0x8000) as *mut u32; // Offset for first hart

        Self { mtime, mtimecmp, msip }
    }

    /// Get mtimecmp for a specific hart
    pub fn mtimecmp(&self, hart: usize) -> *mut u64 {
        // Each mtimecmp is 8 bytes apart
        (self.mtimecmp as usize + hart * 8) as *mut u64
    }

    /// Get msip for a specific hart
    pub fn msip(&self, hart: usize) -> *mut u32 {
        // Each msip is 4 bytes apart
        (self.msip as usize + hart * 4) as *mut u32
    }

    /// Get current time from mtime
    pub fn get_mtime(&self) -> u64 {
        unsafe { *self.mtime }
    }

    /// Set timer for a hart
    pub fn set_timer(&self, hart: usize, value: u64) {
        unsafe {
            *self.mtimecmp(hart) = value;
        }
    }

    /// Send IPI to a hart
    pub fn send_ipi(&self, hart: usize) {
        unsafe {
            *self.msip(hart) = 1;
        }
    }

    /// Clear IPI for a hart
    pub fn clear_ipi(&self, hart: usize) {
        unsafe {
            *self.msip(hart) = 0;
        }
    }

    /// Check if IPI is pending for a hart
    pub fn ipi_pending(&self, hart: usize) -> bool {
        unsafe { *self.msip(hart) != 0 }
    }
}

// ============================================================================
/// RISC-V Memory Barrier Instructions
/// ============================================================================

/// Execute fence instruction (memory barrier)
#[inline]
pub fn fence() {
    unsafe {
        core::arch::asm!("fence", options(nostack));
    }
}

/// Execute fence.i instruction (I/O memory barrier)
#[inline]
pub fn fence_i() {
    unsafe {
        core::arch::asm!("fence.i", options(nostack));
    }
}

/// Execute fence.s instruction (store barrier)
#[inline]
pub fn fence_s() {
    unsafe {
        core::arch::asm!("fence.s", options(nostack));
    }
}

// ============================================================================
/// RISC-V CPU Information
/// ============================================================================

/// RISC-V CPU features
#[derive(Debug, Clone, Copy)]
pub struct RiscvFeatures {
    /// Has atomic instructions (A extension)
    pub atomic: bool,

    /// Has multiply extension (M extension)
    pub mul: bool,

    /// Has divide extension (M extension)
    pub div: bool,

    /// Has atomic 64-bit (A extension)
    pub atomic64: bool,

    /// Has floating point (F extension)
    pub fp: bool,

    /// has double precision floating point (D extension)
    pub double: bool,

    /// Has compressed ISA (C extension)
    pub compressed: bool,
}

impl RiscvFeatures {
    /// Detect RISC-V CPU features
    ///
    /// # Safety
    ///
    /// Must be called from RISC-V code.
    pub unsafe fn detect() -> Self {
        // TODO: Read misa, mvendorid, marchid, mimpid registers
        Self {
            atomic: false,
            mul: false,
            div: false,
            atomic64: false,
            fp: false,
            double: false,
            compressed: true, // C is required
        }
    }
}

/// Get RISC-V CPU features
pub fn get_features() -> RiscvFeatures {
    unsafe { RiscvFeatures::detect() }
}

// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_sizes() {
        assert_eq!(RISCV_PAGE_SIZE, 4096);
        assert_eq!(RISCV_PAGE_SHIFT, 12);
    }

    #[test]
    fn test_sbi_ret() {
        assert_eq!(SbiRet::from_raw(0), SbiRet::Success);
        assert_eq!(SbiRet::from_raw(-1), SbiRet::Failed);
        assert_eq!(SbiRet::from_raw(-2), SbiRet::NotSupported);
        assert_eq!(SbiRet::Success.into_raw(), 0);
    }

    #[test]
    fn test_hart_info() {
        let info = HartInfo::new(1, 1, true);
        assert_eq!(info.hart_id, 1);
        assert!(info.is_bootstrap);
    }

    #[test]
    fn test_interrupt_controller() {
        let controller = RiscvInterruptController::new();
        assert_eq!(controller.max_irq(), 1024);
    }
}
