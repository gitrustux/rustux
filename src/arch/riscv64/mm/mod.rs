// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V Memory Management Unit (MMU)
//!
//! This module provides MMU support for RISC-V systems, including:
//! - Sv39 page table management (39-bit virtual addresses)
//! - Sv48 page table management (48-bit virtual addresses)
//! - Page table entry formats
//! - Address space management

use core::sync::atomic::{AtomicU16, Ordering};

/// ============================================================================
/// RISC-V Page Table Definitions
/// ============================================================================

/// RISC-V page size (4KB)
pub const RISCV_PAGE_SIZE: usize = 4096;

/// RISC-V page shift
pub const RISCV_PAGE_SHIFT: usize = 12;

/// Sv39 virtual address bits
pub const SV39_VA_BITS: usize = 39;

/// Sv48 virtual address bits
pub const SV48_VA_BITS: usize = 48;

/// Sv39 physical address bits (assumed 56-bit)
pub const SV39_PA_BITS: usize = 56;

/// Sv48 physical address bits (assumed 56-bit)
pub const SV48_PA_BITS: usize = 56;

/// ============================================================================
/// Page Table Modes
/// ============================================================================

/// Page table mode
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableMode {
    /// Sv39: 39-bit virtual addresses, 3-level page table
    Sv39 = 0,

    /// Sv48: 48-bit virtual addresses, 4-level page table
    Sv48 = 1,
}

impl PageTableMode {
    /// Get virtual address bits for this mode
    pub const fn va_bits(&self) -> usize {
        match self {
            Self::Sv39 => SV39_VA_BITS,
            Self::Sv48 => SV48_VA_BITS,
        }
    }

    /// Get physical address bits for this mode
    pub const fn pa_bits(&self) -> usize {
        match self {
            Self::Sv39 => SV39_PA_BITS,
            Self::Sv48 => SV48_PA_BITS,
        }
    }

    /// Get number of page table levels
    pub const fn levels(&self) -> usize {
        match self {
            Self::Sv39 => 3,
            Self::Sv48 => 4,
        }
    }
}

/// ============================================================================
/// Page Table Level
/// ============================================================================

/// Page table level
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableLevel {
    /// Level 2: Root table (512GiB regions for Sv39, 256TiB for Sv48)
    L2 = 2,

    /// Level 1: Mid-level (1GiB regions)
    L1 = 1,

    /// Level 0: Mid-level (2MiB regions)
    L0 = 0,

    /// Level -1: Leaf level (4KiB pages)
    Lm1 = -1i8 as u8,
}

impl PageTableLevel {
    /// Get the VPN shift for this level (Sv39)
    pub const fn shift_sv39(&self) -> usize {
        match self {
            Self::L2 => 30, // 1GiB
            Self::L1 => 21, // 2MiB
            Self::L0 => 12, // 4KiB
            Self::Lm1 => 0, // Invalid
        }
    }

    /// Get the VPN shift for this level (Sv48)
    pub const fn shift_sv48(&self) -> usize {
        match self {
            Self::L2 => 39, // 512GiB
            Self::L1 => 30, // 1GiB
            Self::L0 => 21, // 2MiB
            Self::Lm1 => 12, // 4KiB
        }
    }

    /// Get the number of entries at this level
    pub const fn num_entries(&self) -> usize {
        512
    }

    /// Get the descriptor size at this level
    pub const fn entry_size(&self) -> usize {
        8 // 64-bit PTEs
    }
}

/// ============================================================================
/// Page Table Entry
/// ============================================================================

/// Page table entry (PTE) format
///
/// RISC-V PTE format (Sv39/Sv48):
/// - Bits 53-10: Physical frame number (PPN)
/// - Bits 9-8: Reserved
/// - Bit 7: D (Dirty)
/// - Bit 6: A (Accessed)
/// - Bit 5: G (Global)
/// - Bit 4: U (User)
/// - Bit 3: X (Execute)
/// - Bit 2: W (Write)
/// - Bit 1: R (Read)
/// - Bit 0: V (Valid)
#[derive(Debug, Clone, Copy)]
pub struct PageTableEntry(pub usize);

impl PageTableEntry {
    /// Invalid entry
    pub const INVALID: Self = Self(0);

    /// Create a page table entry
    ///
    /// # Arguments
    ///
    /// * `ppn` - Physical page number
    /// * `flags` - Entry flags
    pub const fn new(ppn: usize, flags: PageTableFlags) -> Self {
        Self((ppn << 10) | (flags.0 & 0xFF))
    }

    /// Check if entry is valid
    pub fn is_valid(&self) -> bool {
        self.0 & 0x1 != 0
    }

    /// Check if entry is readable
    pub fn is_readable(&self) -> bool {
        self.0 & 0x2 != 0
    }

    /// Check if entry is writable
    pub fn is_writable(&self) -> bool {
        self.0 & 0x4 != 0
    }

    /// Check if entry is executable
    pub fn is_executable(&self) -> bool {
        self.0 & 0x8 != 0
    }

    /// Check if entry is user-accessible
    pub fn is_user(&self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Check if entry is global
    pub fn is_global(&self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Check if entry has been accessed
    pub fn is_accessed(&self) -> bool {
        self.0 & 0x40 != 0
    }

    /// Check if entry is dirty
    pub fn is_dirty(&self) -> bool {
        self.0 & 0x80 != 0
    }

    /// Get physical page number
    pub fn ppn(&self) -> usize {
        self.0 >> 10
    }

    /// Get physical address
    pub fn phys_addr(&self) -> usize {
        self.ppn() << RISCV_PAGE_SHIFT
    }

    /// Set accessed flag
    pub fn set_accessed(&mut self) {
        self.0 |= 0x40;
    }

    /// Set dirty flag
    pub fn set_dirty(&mut self) {
        self.0 |= 0x80;
    }
}

/// ============================================================================
/// Page Table Flags
/// ============================================================================

/// Page table entry flags
#[derive(Debug, Clone, Copy)]
pub struct PageTableFlags(pub usize);

impl PageTableFlags {
    /// No flags
    pub const NONE: Self = Self(0);

    /// Valid bit
    pub const V: Self = Self(1 << 0);

    /// Read bit
    pub const R: Self = Self(1 << 1);

    /// Write bit
    pub const W: Self = Self(1 << 2);

    /// Execute bit
    pub const X: Self = Self(1 << 3);

    /// User accessible
    pub const U: Self = Self(1 << 4);

    /// Global mapping
    pub const G: Self = Self(1 << 5);

    /// Accessed flag
    pub const A: Self = Self(1 << 6);

    /// Dirty flag
    pub const D: Self = Self(1 << 7);

    /// Read/Write for kernel
    pub const KERNEL_RW: Self = Self(Self::V.0 | Self::R.0 | Self::W.0 | Self::X.0 | Self::G.0 | Self::A.0 | Self::D.0);

    /// Read/Write/Execute for user
    pub const USER_RWX: Self = Self(Self::V.0 | Self::R.0 | Self::W.0 | Self::X.0 | Self::U.0 | Self::A.0 | Self::D.0);

    /// Read-only for user
    pub const USER_R: Self = Self(Self::V.0 | Self::R.0 | Self::U.0 | Self::A.0);

    /// Read-only for kernel
    pub const KERNEL_R: Self = Self(Self::V.0 | Self::R.0 | Self::G.0 | Self::A.0);
}

/// ============================================================================
/// Page Table
/// ============================================================================

/// RISC-V page table
pub struct PageTable {
    /// Physical address of the table
    pub phys: usize,

    /// Virtual address (for kernel mapping)
    pub virt: usize,

    /// Table mode (Sv39 or Sv48)
    pub mode: PageTableMode,

    /// Table level
    pub level: PageTableLevel,
}

impl PageTable {
    /// Create a new page table
    ///
    /// # Arguments
    ///
    /// * `phys` - Physical address of the table
    /// * `virt` - Virtual address (for kernel mapping)
    /// * `mode` - Page table mode (Sv39 or Sv48)
    /// * `level` - Table level
    pub const fn new(phys: usize, virt: usize, mode: PageTableMode, level: PageTableLevel) -> Self {
        Self { phys, virt, mode, level }
    }

    /// Get the level
    pub const fn level(&self) -> PageTableLevel {
        self.level
    }

    /// Get the physical address
    pub const fn phys(&self) -> usize {
        self.phys
    }

    /// Get the virtual address
    pub const fn virt(&self) -> usize {
        self.virt
    }

    /// Get the mode
    pub const fn mode(&self) -> PageTableMode {
        self.mode
    }
}

/// ============================================================================
/// Address Space
/// ============================================================================

/// Address space identifier
pub type Asid = u16;

/// Invalid ASID
pub const ASID_INVALID: Asid = u16::MAX;

/// Kernel ASID
pub const ASID_KERNEL: Asid = 0;

/// Address space
pub struct AddressSpace {
    /// Root page table physical address
    pub root_page_table: usize,

    /// Root page table virtual address
    pub root_page_table_virt: usize,

    /// ASID
    pub asid: Asid,

    /// Page table mode
    pub mode: PageTableMode,
}

impl AddressSpace {
    /// Create a new address space
    ///
    /// # Arguments
    ///
    /// * `root_page_table` - Physical address of root page table
    /// * `root_page_table_virt` - Virtual address of root page table
    /// * `mode` - Page table mode
    pub fn new(root_page_table: usize, root_page_table_virt: usize, mode: PageTableMode) -> Self {
        Self {
            root_page_table,
            root_page_table_virt,
            asid: ASID_INVALID,
            mode,
        }
    }

    /// Set ASID
    pub fn set_asid(&mut self, asid: Asid) {
        self.asid = asid;
    }

    /// Get ASID
    pub fn asid(&self) -> Asid {
        self.asid
    }
}

/// ============================================================================
/// ASID Allocator
/// ============================================================================

/// ASID (Address Space ID) allocator
pub struct AsidAllocator {
    /// Next ASID to allocate
    next: AtomicU16,

    /// Maximum ASID (typically 2^16 - 2 for RISC-V)
    max: Asid,
}

impl AsidAllocator {
    /// Create a new ASID allocator
    ///
    /// # Arguments
    ///
    /// * `max` - Maximum ASID value (default: 65534)
    pub const fn new(max: Asid) -> Self {
        Self {
            next: AtomicU16::new(1), // Start at 1 (0 is kernel)
            max,
        }
    }

    /// Allocate a new ASID
    pub fn alloc(&self) -> Option<Asid> {
        let asid = self.next.fetch_add(1, Ordering::Relaxed);
        if asid > self.max || asid == ASID_INVALID {
            None
        } else {
            Some(asid)
        }
    }

    /// Reset the allocator
    pub fn reset(&self) {
        self.next.store(1, Ordering::Release);
    }
}

/// ============================================================================
/// TLB Management
/// ============================================================================

/// Execute SFENCE.VMA instruction (flush TLB entry)
///
/// # Safety
///
/// Must be called from RISC-V code.
#[inline]
pub unsafe fn sfence_vma() {
    core::arch::asm!("sfence.vma", options(nostack));
}

/// Execute SFENCE.VMA instruction with ASID
///
/// # Arguments
///
/// * `asid` - Address space ID to flush
///
/// # Safety
///
/// Must be called from RISC-V code.
#[inline]
pub unsafe fn sfence_vma_asid(asid: Asid) {
    let _ = asid;
    core::arch::asm!("sfence.vma zero, {}", in(reg) asid, options(nostack));
}

/// Execute SFENCE.VMA instruction for specific address
///
/// # Arguments
///
/// * `addr` - Virtual address to flush
///
/// # Safety
///
/// Must be called from RISC-V code.
#[inline]
pub unsafe fn sfence_vma_addr(addr: usize) {
    let _ = addr;
    core::arch::asm!("sfence.vma {}, zero", in(reg) addr, options(nostack));
}

/// ============================================================================
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
    fn test_page_table_mode() {
        assert_eq!(PageTableMode::Sv39.va_bits(), 39);
        assert_eq!(PageTableMode::Sv48.va_bits(), 48);
        assert_eq!(PageTableMode::Sv39.levels(), 3);
        assert_eq!(PageTableMode::Sv48.levels(), 4);
    }

    #[test]
    fn test_page_table_entry() {
        let pte = PageTableEntry::new(0x12345, PageTableFlags::KERNEL_RW);

        assert!(pte.is_valid());
        assert!(pte.is_readable());
        assert!(pte.is_writable());
        assert!(pte.is_executable());
        assert!(pte.is_global());
        assert!(!pte.is_user());

        assert_eq!(pte.ppn(), 0x12345);
    }

    #[test]
    fn test_page_table_flags() {
        let flags = PageTableFlags::KERNEL_RW;
        assert!(flags.0 & PageTableFlags::V.0 != 0);
        assert!(flags.0 & PageTableFlags::R.0 != 0);
        assert!(flags.0 & PageTableFlags::W.0 != 0);
    }

    #[test]
    fn test_asid_allocator() {
        let alloc = AsidAllocator::new(100);
        assert_eq!(alloc.alloc(), Some(1));
        assert_eq!(alloc.alloc(), Some(2));
    }

    #[test]
    fn test_address_space() {
        let mut space = AddressSpace::new(0x80000, 0xFFFF80000000, PageTableMode::Sv39);
        assert_eq!(space.asid(), ASID_INVALID);

        space.set_asid(5);
        assert_eq!(space.asid(), 5);
    }

    #[test]
    fn test_page_table_level_shift() {
        assert_eq!(PageTableLevel::L2.shift_sv39(), 30);
        assert_eq!(PageTableLevel::L1.shift_sv39(), 21);
        assert_eq!(PageTableLevel::L0.shift_sv39(), 12);

        assert_eq!(PageTableLevel::L2.shift_sv48(), 39);
        assert_eq!(PageTableLevel::L1.shift_sv48(), 30);
    }
}
