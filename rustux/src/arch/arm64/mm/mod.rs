// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses.org/LICENSE

//! ARM64 Memory Management Unit (MMU)
//!
//! This module provides MMU support for ARM64 systems, including:
//! - Page table management (4-level and 2-level formats)
//! - Translation table descriptors
//! - Address space management
//! - TLB maintenance

use core::sync::atomic::{AtomicU16, Ordering};

/// Physical address type
pub type PAddr = u64;

/// ============================================================================
/// ARM64 Page Table Definitions
/// ============================================================================

/// ARM64 page size (4KB)
pub const ARM64_PAGE_SIZE: usize = 4096;

/// ARM64 page size shift
pub const ARM64_PAGE_SHIFT: usize = 12;

/// ARM64 virtual address bits (48-bit VMSA)
pub const ARM64_VA_BITS: usize = 48;

/// ARM64 physical address bits (40-bit for now)
pub const ARM64_PA_BITS: usize = 40;

/// ARM64 maximum virtual address
pub const ARM64_MAX_VA: usize = (1usize << ARM64_VA_BITS) - 1;

/// ARM64 maximum physical address
pub const ARM64_MAX_PA: usize = (1usize << ARM64_PA_BITS) - 1;

/// ============================================================================
/// Page Table Level
/// ============================================================================

/// Page table level
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableLevel {
    /// Level 0: Translation Table (512GB regions)
    L0 = 0,

    /// Level 1: First-level table (1GB blocks)
    L1 = 1,

    /// Level 2: Second-level table (2MB blocks)
    L2 = 2,

    /// Level 3: Third-level table (4KB pages)
    L3 = 3,
}

impl PageTableLevel {
    /// Get the table shift for this level
    pub const fn shift(&self) -> usize {
        match self {
            Self::L0 => 39, // 512GB regions
            Self::L1 => 30, // 1GB blocks
            Self::L2 => 21, // 2MB blocks
            Self::L3 => 12, // 4KB pages
        }
    }

    /// Get the number of entries at this level
    pub const fn num_entries(&self) -> usize {
        512
    }

    /// Get the descriptor size at this level
    pub const fn descriptor_size(&self) -> usize {
        8 // 64-bit descriptors
    }
}

/// ============================================================================
/// Page Table Descriptor Types
/// ============================================================================

/// Page table descriptor type
#[derive(Debug, Clone, Copy)]
pub struct PageTableDescriptor(pub u64);

impl PageTableDescriptor {
    /// Invalid descriptor
    pub const INVALID: Self = Self(0);

    /// Block descriptor (mapping)
    pub const BLOCK: Self = Self(0x1);

    /// Table descriptor (points to next level)
    pub const TABLE: Self = Self(0x3);

    /// Page descriptor (4KB page)
    pub const PAGE: Self = Self(0x3);

    /// Create a block descriptor
    ///
    /// # Arguments
    ///
    /// * `output_address` - Physical address to map
    /// * `flags` - Attribute flags
    pub const fn block(output_address: u64, flags: DescriptorFlags) -> Self {
        Self(output_address | flags.0)
    }

    /// Create a table descriptor
    ///
    /// # Arguments
    ///
    /// * `next_table_address` - Physical address of next level table
    pub const fn table(next_table_address: u64) -> Self {
        Self(next_table_address | 0x3)
    }

    /// Create a page descriptor
    ///
    /// # Arguments
    ///
    /// * `output_address` - Physical address of page
    /// * `flags` - Attribute flags
    pub const fn page(output_address: u64, flags: DescriptorFlags) -> Self {
        Self(output_address | flags.0)
    }

    /// Check if descriptor is valid
    pub fn is_valid(&self) -> bool {
        self.0 & 0x1 != 0
    }

    /// Check if descriptor is a table
    pub fn is_table(&self) -> bool {
        self.0 & 0x3 == 0x3 && (self.0 & 0x1) == 0
    }

    /// Get output address
    pub fn output_address(&self) -> u64 {
        self.0 & 0x0000_FFFF_FFFF_FFFF
    }
}

/// ============================================================================
/// Descriptor Flags
/// ============================================================================

/// Page table descriptor flags
#[derive(Debug, Clone, Copy)]
pub struct DescriptorFlags(pub u64);

impl DescriptorFlags {
    /// No flags (invalid)
    pub const NONE: Self = Self(0);

    /// Block descriptor (mapping)
    pub const BLOCK: Self = Self(0x1);

    /// Access flag (AF)
    pub const AF: Self = Self(1 << 10);

    /// Shareable flag (SH)
    pub const SH_INNER: Self = Self(0 << 8);
    pub const SH_OUTER: Self = Self(2 << 8);
    pub const SH: Self = Self(3 << 8);

    /// Access Permissions (AP)
    pub const AP_RO: Self = Self(0 << 6);
    pub const AP_RW: Self = Self(1 << 6);
    pub const AP_RW_EL0: Self = Self(1 << 6);
    pub const AP_RO_EL0: Self = Self(3 << 6);

    /// User/Kernel access
    pub const AP_USER: Self = Self(1 << 6); // EL0

    /// Global flag
    pub const NG: Self = Self(1 << 11); // Non-global

    /// Contiguous hint
    pub const CONTIGUOUS: Self = Self(1 << 52);

    /// Privileged execute-never
    pub const PXN: Self = Self(1 << 53);

    /// Unprivileged execute-never
    pub const UXN: Self = Self(1 << 54);

    /// Execute-never
    pub const XN: Self = Self(1 << 54);

    /// Dirty bit (modified)
    pub const DBM: Self = Self(1 << 51);

    /// Default flags for kernel memory
    pub const KERNEL: Self = Self(
        Self::BLOCK.0 |
        Self::AF.0 |
        Self::SH_INNER.0 |
        Self::AP_RW.0 |
        Self::PXN.0,
    );

    /// Default flags for user memory
    pub const USER: Self = Self(
        Self::BLOCK.0 |
        Self::AF.0 |
        Self::SH_OUTER.0 |
        Self::AP_USER.0 |
        Self::UXN.0,
    );

    /// Create block descriptor flags for kernel memory
    pub const fn kernel_block(output_address: u64) -> u64 {
        PageTableDescriptor::block(output_address, Self::KERNEL).0
    }
}

/// ============================================================================
/// ARM64 Page Table
/// ============================================================================

/// ARM64 page table
pub struct PageTable {
    /// Physical address of the table
    pub phys: u64,

    /// Virtual address (for kernel mapping)
    pub virt: usize,

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
    /// * `level` - Table level
    pub const fn new(phys: u64, virt: usize, level: PageTableLevel) -> Self {
        Self { phys, virt, level }
    }

    /// Get the level
    pub const fn level(&self) -> PageTableLevel {
        self.level
    }

    /// Get the physical address
    pub const fn phys(&self) -> u64 {
        self.phys
    }

    /// Get the virtual address
    pub const fn virt(&self) -> usize {
        self.virt
    }
}

/// ============================================================================
/// Translation Context
/// ============================================================================

/// Translation context for address space management
pub struct TranslationContext {
    /// Root table physical address
    pub root_table: u64,

    /// Root table virtual address
    pub root_table_virt: usize,

    /// ASID (Address Space ID)
    pub asid: AtomicU16,
}

impl TranslationContext {
    /// Create a new translation context
    pub fn new(root_table: u64, root_table_virt: usize) -> Self {
        Self {
            root_table,
            root_table_virt,
            asid: AtomicU16::new(0),
        }
    }

    /// Get the root table address
    pub fn root_table(&self) -> u64 {
        self.root_table
    }

    /// Get the root table virtual address
    pub fn root_table_virt(&self) -> usize {
        self.root_table_virt
    }

    /// Allocate ASID
    pub fn alloc_asid(&self) -> u16 {
        self.asid.fetch_add(1, Ordering::Relaxed)
    }

    /// Free ASID
    pub fn free_asid(&self, asid: u16) {
        // TODO: Implement ASID tracking
        let _ = asid;
    }
}

// ============================================================================
/// Tests
/// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_sizes() {
        assert_eq!(ARM64_PAGE_SIZE, 4096);
        assert_eq!(ARM64_PAGE_SHIFT, 12);
        assert_eq!(ARM64_VA_BITS, 48);
    }

    #[test]
    fn test_page_table_level() {
        assert_eq!(PageTableLevel::L0.shift(), 39);
        assert_eq!(PageTableLevel::L1.shift(), 30);
        assert_eq!(PageTableLevel::L2.shift(), 21);
        assert_eq!(PageTableLevel::L3.shift(), 12);

        assert_eq!(PageTableLevel::L0.num_entries(), 512);
        assert_eq!(PageTableLevel::L1.num_entries(), 512);
    }

    #[test]
    fn test_descriptor() {
        let flags = DescriptorFlags::KERNEL;
        let desc = PageTableDescriptor::block(0x1000, flags);

        assert!(desc.is_valid());
        assert_eq!(desc.output_address(), 0x1000);
    }

    #[test]
    fn test_descriptor_flags() {
        assert!(DescriptorFlags::KERNEL.0 != 0);
        assert!(DescriptorFlags::USER.0 != 0);
        assert!((DescriptorFlags::KERNEL.0 & DescriptorFlags::USER.0) == 0);
    }
}
