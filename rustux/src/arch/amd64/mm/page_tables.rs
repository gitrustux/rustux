// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 Page Table Management
//!
//! This module provides page table structures for x86-64.

/// Page table entry type (64-bit PTE)
pub type pt_entry_t = u64;

/// Physical address type
pub type PAddr = u64;

/// Virtual address type
pub type VAddr = usize;

/// Page size in bytes
pub const PAGE_SIZE: usize = 1 << 12; // 4096 bytes

/// Number of entries per page table
pub const ENTRIES_PER_PAGE_TABLE: usize = 512;

/// Number of entries per page table
pub const PAGE_SIZE_SHIFT: usize = 12;

/// Different page table levels in the 4-level paging hierarchy
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageTableLevel {
    /// Page Table level (4K pages)
    PT_L = 0,
    /// Page Directory level (2M pages)
    PD_L = 1,
    /// Page Directory Pointer Table level (1G pages)
    PDP_L = 2,
    /// Page Map Level 4 (top level)
    PML4_L = 3,
}

/// Page table role for unified address spaces
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTableRole {
    /// Independent page table
    Independent = 0,
    /// Restricted page table (part of unified aspace)
    Restricted = 1,
    /// Shared page table (part of unified aspace)
    Shared = 2,
    /// Unified page table (combines restricted + shared)
    Unified = 3,
}

/// Type for flags used in the hardware page tables
pub type PtFlags = u64;

/// Type for flags used in the hardware page tables
pub type IntermediatePtFlags = u64;

// ============================================================================
// Page Table Entry Flags (x86_64)
// ============================================================================

/// Page table entry flags module
pub mod mmu_flags {
    /// Present flag - page is mapped
    pub const X86_MMU_PG_P: u64 = 0x0001;

    /// Read/Write flag
    pub const X86_MMU_PG_RW: u64 = 0x0002;

    /// User/Supervisor flag
    pub const X86_MMU_PG_U: u64 = 0x0004;

    /// Write-Through cache flag
    pub const X86_MMU_PG_WT: u64 = 0x0008;

    /// Cache Disable flag
    pub const X86_MMU_PG_CD: u64 = 0x0010;

    /// Accessed flag
    pub const X86_MMU_PG_A: u64 = 0x0020;

    /// Dirty flag
    pub const X86_MMU_PG_D: u64 = 0x0040;

    /// Page Size flag (1=2MB/1GB, 0=4KB)
    pub const X86_MMU_PG_PS: u64 = 0x0080;

    /// Global flag
    pub const X86_MMU_PG_G: u64 = 0x0100;

    /// PAT flag for 4KB pages
    pub const X86_MMU_PG_PTE_PAT: u64 = 0x0080;

    /// PAT flag for large pages
    pub const X86_MMU_PG_LARGE_PAT: u64 = 0x1000;

    /// NX (No-Execute) bit (only in EPT)
    pub const X86_EPT_X: u64 = 0x00000001;

    /// EPT Read flag
    pub const X86_EPT_R: u64 = 0x00000002;

    /// EPT Write flag
    pub const X86_EPT_W: u64 = 0x00000004;

    /// EPT Write-Back memory type
    pub const X86_EPT_WB: u64 = 0x00000006;

    /// EPT attributes
    pub const X86_EPT_EXECUTE_DISABLE: u64 = 0x00000001;

    /// EPT memory types
    pub const X86_EPT_UNCACHEABLE: u64 = 0x00000000;

    /// EPT PAT memory types
    pub const X86_EPT_MEMORY_TYPE_MASK: u64 = 0x38; // Bits 3:5

    /// EPT PAT write-back caching
    pub const X86_EPT_WRITE_BACK: u64 = 0x6; // Bits 1:0x38 = 110b

    /// EPT PAT write-combining cache
    pub const X86_EPT_WRITE_COMBINING: u64 = 0;
}

/// Status return type
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxStatus {
    /// Operation completed successfully
    OK = 0,
    /// Invalid argument
    ERR_INVALID_ARGS = 1,
    /// No memory available
    ERR_NO_MEMORY = 2,
    /// Not implemented
    ERR_NOT_IMPLEMENTED = 3,
    /// Access denied
    ERR_ACCESS_DENIED = 4,
    /// Resource not found
    ERR_NOT_FOUND = 5,
    /// Resource busy
    ERR_BUSY = 6,
    /// IO Error
    ERR_IO = 7,
    /// Internal error
    ERR_INTERNAL = 8,
    /// Not supported
    ERR_NOT_SUPPORTED = 9,
}

/// Result type using RxStatus
pub type RxResult<T> = Result<T, RxStatus>;

/// Base class for x86 page tables
///
/// This provides the common interface for page table operations.
pub struct X86PageTableBase {
    /// Physical address of the page table
    pub phys: PAddr,
    /// Virtual address of the page table
    pub virt: *mut pt_entry_t,
    /// Number of pages allocated for this page table
    pub pages: usize,
    /// Role of this page table (for unified address spaces)
    pub role: PageTableRole,
    /// Number of references to this page table (for unified address spaces)
    pub num_references: u32,
}

impl X86PageTableBase {
    /// Create a new empty page table base
    pub const fn new() -> Self {
        Self {
            phys: 0,
            virt: core::ptr::null_mut(),
            pages: 0,
            role: PageTableRole::Independent,
            num_references: 0,
        }
    }

    /// Get the physical address of this page table
    pub fn phys(&self) -> PAddr {
        self.phys
    }

    /// Get the virtual address of this page table
    pub fn virt(&self) -> *mut pt_entry_t {
        self.virt
    }

    /// Get the number of pages allocated for this page table
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Check if this page table is restricted
    pub fn is_restricted(&self) -> bool {
        self.role == PageTableRole::Restricted
    }

    /// Check if this page table is shared
    pub fn is_shared(&self) -> bool {
        self.role == PageTableRole::Shared
    }

    /// Check if this page table is unified
    pub fn is_unified(&self) -> bool {
        self.role == PageTableRole::Unified
    }

    /// Get the lock order for this page table
    pub fn lock_order(&self) -> u32 {
        if self.is_unified() { 1 } else { 0 }
    }

    /// Initialize the page table with a context pointer
    ///
    /// # Arguments
    ///
    /// * `ctx` - Context pointer for TLB invalidation
    ///
    /// # Returns
    ///
    /// Status code indicating success or failure
    pub fn init(&mut self, _ctx: *mut u8) -> RxStatus {
        // TODO: Implement context initialization
        RxStatus::OK
    }

    /// Destroy the page table
    ///
    /// # Returns
    ///
    /// Status code indicating success or failure
    pub fn destroy(&mut self) -> RxStatus {
        // TODO: Implement page table cleanup
        self.virt = core::ptr::null_mut();
        self.phys = 0;
        self.pages = 0;
        RxStatus::OK
    }
}

/// Page table entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PageTableEntry {
    pub value: u64,
}

/// Page table entry (for compatibility)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PtEntry {
    pub value: u64,
}

impl PageTableEntry {
    /// Create a null page table entry
    pub const fn null() -> Self {
        Self { value: 0 }
    }

    /// Create a new page table entry with the given value
    pub fn new(value: u64) -> Self {
        Self { value }
    }

    /// Get the raw value of the page table entry
    pub fn raw(&self) -> u64 {
        self.value
    }

    /// Set a new value for the page table entry
    pub fn set(&mut self, value: u64) {
        self.value = value;
    }

    /// Get whether the page is present
    pub fn is_present(&self) -> bool {
        self.value & mmu_flags::X86_MMU_PG_P as u64 != 0
    }

    /// Get whether the page is writable
    pub fn is_writable(&self) -> bool {
        self.value & mmu_flags::X86_MMU_PG_RW as u64 != 0
    }

    /// Get whether the page is user-accessible
    pub fn is_user(&self) -> bool {
        self.value & mmu_flags::X86_MMU_PG_U as u64 != 0
    }

    /// Check if this is a large page
    pub fn is_large_page(&self) -> bool {
        self.value & mmu_flags::X86_MMU_PG_PS as u64 != 0
    }

    /// Set the present bit
    pub fn set_present(&mut self) {
        self.value |= mmu_flags::X86_MMU_PG_P;
    }

    /// Clear the present bit
    pub fn clear_present(&mut self) {
        self.value &= !mmu_flags::X86_MMU_PG_P;
    }

    /// Set the writable bit
    pub fn set_writable(&mut self) {
        self.value |= mmu_flags::X86_MMU_PG_RW;
    }

    /// Clear the writable bit
    pub fn clear_writable(&mut self) {
        self.value &= !mmu_flags::X86_MMU_PG_RW;
    }

    /// Set the user bit
    pub fn set_user(&mut self) {
        self.value |= mmu_flags::X86_MMU_PG_U;
    }

    /// Clear the user bit
    pub fn clear_user(&mut self) {
        self.value &= !mmu_flags::X86_MMU_PG_U;
    }

    /// Set the dirty bit
    pub fn set_dirty(&mut self) {
        self.value |= mmu_flags::X86_MMU_PG_D;
    }

    /// Get the physical address from a PTE
    pub fn phys(&self) -> PAddr {
        self.value & 0x000FFFFFFFFF000
    }

    /// Set the physical address in the PTE
    pub fn set_phys(&mut self, phys: PAddr) {
        // Clear the physical address field (bits 12-51)
        self.value &= !0x000FFFFFFFF000;
        // Set the new physical address
        self.value |= phys & 0x000FFFFFFFF000;
    }

    /// Get the virtual address from a PTE
    pub fn vaddr(&self) -> VAddr {
        const ADDR_MASK: u64 = 0x0007FFFFFFFFFFF;
        ((self.value & ADDR_MASK) as u64) as VAddr
    }

    /// Set the virtual address in the PTE
    pub fn set_vaddr(&mut self, vaddr: VAddr) {
        // Clear the virtual address field (bits 12-51)
        self.value &= !0x0007FFFFFFFFFFF;
        // Set the new virtual address
        self.value |= (vaddr & 0x0007FFFFFFFFFFF) as u64;
    }
}

/// Initialize a page table with zeroed entries
///
/// # Safety
///
/// Caller must ensure the virt pointer points to valid memory and page_count is valid
pub unsafe fn init_page_table(virt: *mut pt_entry_t, page_count: usize) {
    for i in 0..page_count {
        *virt.add(i) = 0;
    }
}

/// Zero a page
///
/// # Safety
///
/// Caller must ensure page_addr points to a valid page-aligned memory region
pub unsafe fn zero_page(page_addr: VAddr) {
    let page_ptr = page_addr as *mut u8;
    for i in 0..PAGE_SIZE {
        *page_ptr.add(i) = 0;
    }
}
