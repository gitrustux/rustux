// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Address Space Implementation
//!
//! This module provides address space management for processes.
//! Each process has its own address space with page tables.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU64, Ordering};
use alloc::collections::BTreeMap;
use crate::sync::SpinMutex;
use crate::object::{Vmo, VmoId};

use crate::arch::amd64::mm::page_tables::{
    X86PageTableBase, PageTableEntry, PageTableRole, PageTableLevel,
    PAddr, VAddr, pt_entry_t,
};

// Page size
const PAGE_SIZE: usize = 4096;

// Page table indices from virtual address
fn pml4_index(vaddr: VAddr) -> usize {
    (vaddr >> 39) & 0x1FF
}

fn pdp_index(vaddr: VAddr) -> usize {
    (vaddr >> 30) & 0x1FF
}

fn pd_index(vaddr: VAddr) -> usize {
    (vaddr >> 21) & 0x1FF
}

fn pt_index(vaddr: VAddr) -> usize {
    (vaddr >> 12) & 0x1FF
}

/// Mapping information for a VMO in this address space
struct VmoMapping {
    /// VMO being mapped
    vmo: Vmo,
    /// Virtual address where VMO is mapped
    vaddr: u64,
    /// Size of mapping
    size: u64,
    /// Mapping permissions (R, W, X)
    flags: u32,
}

/// Address Space
///
/// Represents a process's virtual address space with page tables.
pub struct AddressSpace {
    /// Address space ID
    id: u64,

    /// Root page table (PML4)
    pub page_table: X86PageTableBase,

    /// Mappings: virtual address -> mapping info
    mappings: SpinMutex<BTreeMap<u64, VmoMapping>>,

    /// Reference count
    ref_count: AtomicU64,
}

/// Next address space ID counter
static mut NEXT_AS_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new address space ID
fn alloc_as_id() -> u64 {
    unsafe { NEXT_AS_ID.fetch_add(1, Ordering::Relaxed) }
}

/// Allocate a page for page tables from the kernel zone
fn alloc_table_page() -> Option<u64> {
    crate::mm::pmm::pmm_alloc_kernel_page().ok()
}

impl AddressSpace {
    /// Create a new address space
    ///
    /// # Returns
    ///
    /// New address space with empty page tables
    pub fn new() -> Result<Self, &'static str> {
        use crate::arch::amd64::mm::kernel_map;

        // Kernel half shared with the kernel, private lower half containing
        // only the (supervisor-only) kernel image.
        let pml4_paddr = unsafe { kernel_map::new_user_pml4(alloc_table_page)? };
        let pml4_vaddr = kernel_map::phys_to_virt(pml4_paddr) as *mut pt_entry_t;

        let page_table = X86PageTableBase {
            phys: pml4_paddr,
            virt: pml4_vaddr,
            pages: 1,
            role: PageTableRole::Independent,
            num_references: 0,
        };

        Ok(Self {
            id: alloc_as_id(),
            page_table,
            mappings: SpinMutex::new(BTreeMap::new()),
            ref_count: AtomicU64::new(1),
        })
    }

    /// Get address space ID
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Map a VMO into this address space
    ///
    /// # Arguments
    ///
    /// * `vmo` - VMO to map
    /// * `vaddr` - Virtual address to map at
    /// * `size` - Size of mapping
    /// * `flags` - Segment permissions (PF_R, PF_W, PF_X)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Mapping successful
    /// * `Err(&str)` - Mapping failed
    pub fn map_vmo(
        &self,
        vmo: &Vmo,
        vaddr: u64,
        size: u64,
        flags: u32,
    ) -> Result<(), &'static str> {
        // Validate alignment
        if vaddr & 0xFFF != 0 {
            unsafe {
                let msg = b"[MAP] ALIGN FAIL vaddr=0x";
                for &b in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
                let mut n = vaddr;
                let mut buf = [0u8; 16];
                let mut i = 0;
                loop {
                    let digit = (n & 0xF) as u8;
                    buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                    n >>= 4;
                    i += 1;
                    if n == 0 { break; }
                }
                while i > 0 {
                    i -= 1;
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                }
                let msg = b"\n";
                for &b in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
            }
            return Err("Virtual address not page-aligned");
        }

        let num_pages = (size as usize + PAGE_SIZE - 1) / PAGE_SIZE;

        // Lock the VMO's pages
        let vmo_pages = vmo.pages.lock();

        // Map each page directly - no intermediate storage needed
        for page_idx in 0..num_pages {
            let page_vaddr = vaddr as usize + page_idx * PAGE_SIZE;
            let page_offset = page_idx * PAGE_SIZE;

            // Get the physical page from the VMO
            let page_entry = vmo_pages.get(&page_offset);

            let paddr = match page_entry {
                Some(entry) => {
                    if !entry.present {
                        return Err("VMO page not present");
                    }
                    entry.paddr
                }
                None => {
                    return Err("VMO page not present");
                }
            };

            self.map_page(page_vaddr as u64, paddr, flags)?;
        }
        // Lock is released here

        // Store the mapping - skip VMO cloning for now to avoid corruption
        // TODO: Fix VMO clone corruption and re-enable cloning
        // For now, we just store a minimal placeholder since we don't need
        // to keep the VMO for the basic userspace execution test
        //let vmo_clone = vmo.clone().map_err(|_| "Failed to clone VMO for mapping")?;
        //let mapping = VmoMapping {
        //    vmo: vmo_clone,
        //    vaddr,
        //    size,
        //    flags,
        //};
        //self.mappings.lock().insert(vaddr, mapping);

        Ok(())
    }

    /// Map a single user page
    ///
    /// # Arguments
    ///
    /// * `vaddr` - Virtual address (must be page-aligned, lower half)
    /// * `paddr` - Physical address (must be page-aligned)
    /// * `flags` - ELF segment flags (PF_X = 1, PF_W = 2, PF_R = 4)
    fn map_page(&self, vaddr: u64, paddr: PAddr, flags: u32) -> Result<(), &'static str> {
        use crate::arch::amd64::mm::kernel_map::{self, PTE_USER, PTE_WRITABLE};

        let mut pte_flags = PTE_USER;
        if flags & 0x2 != 0 {
            pte_flags |= PTE_WRITABLE;
        }
        unsafe { kernel_map::map_4k(self.page_table.phys, vaddr, paddr, pte_flags, alloc_table_page) }
    }

    /// Activate this address space
    ///
    /// Loads the page table into CR3, making it the active address space.
    ///
    /// # Safety
    ///
    /// This switches the entire address space. The caller must ensure
    /// that the current execution context is properly set up for the switch.
    pub unsafe fn activate(&self) {
        use crate::arch::amd64::init::x86_write_cr3;

        // Load CR3 with the physical address of the PML4
        x86_write_cr3(self.page_table.phys);
    }
}

/// Default implementation for the old placeholder AddressSpace
impl Default for AddressSpace {
    fn default() -> Self {
        // Create a new address space
        Self::new().expect("Failed to create default address space")
    }
}
