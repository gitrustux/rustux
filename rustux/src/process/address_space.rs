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

impl AddressSpace {
    /// Create a new address space
    ///
    /// # Returns
    ///
    /// New address space with empty page tables
    pub fn new() -> Result<Self, &'static str> {
        use crate::mm::pmm;
        use crate::arch::amd64::init;

        // Allocate a page for the PML4 from kernel zone
        let pml4_paddr = pmm::pmm_alloc_kernel_page()
            .map_err(|_| "Failed to allocate PML4 page")?;

        let pml4_vaddr = pmm::paddr_to_vaddr(pml4_paddr) as *mut pt_entry_t;

        // Initialize the page table structure
        let page_table = X86PageTableBase {
            phys: pml4_paddr,
            virt: pml4_vaddr,
            pages: 1,
            role: PageTableRole::Independent,
            num_references: 0,
        };

        // Zero the PML4
        unsafe {
            let pml4_bytes = core::slice::from_raw_parts_mut(
                pml4_vaddr as *mut u8,
                PAGE_SIZE,
            );
            pml4_bytes.fill(0);
        }

        // CRITICAL: Copy ALL kernel PML4 entries (0-511) to process page table
        // This ensures that when we switch CR3, the kernel code remains accessible
        // The kernel code is executing at low addresses (identity-mapped), so we need
        // to copy all entries, not just the higher-half entries.

        unsafe {
            let kernel_cr3 = init::x86_read_cr3();
            let kernel_pml4_paddr = kernel_cr3 & !0xFFF;
            let kernel_pml4_vaddr = pmm::paddr_to_vaddr(kernel_pml4_paddr) as *const pt_entry_t;

            // First, copy low address entries (0-255) for kernel identity mapping
            for i in 0..256 {
                let entry = *kernel_pml4_vaddr.add(i);
                // Copy the entry to process page table
                *pml4_vaddr.add(i) = entry;
            }

            // Then, copy higher-half entries (256-511) for kernel higher-half mappings
            for i in 256..512 {
                let entry = *kernel_pml4_vaddr.add(i);
                // Copy the entry to process page table
                *pml4_vaddr.add(i) = entry;
            }
        }

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

        // Disable interrupts during mapping to prevent interference
        let interrupt_flags = unsafe { crate::arch::amd64::init::arch_disable_ints() };

        {
            // Lock the VMO's pages
            let vmo_pages = vmo.pages.lock();

            // Map each page directly - no intermediate storage needed
            // This avoids the 256-page limit and buffer overflow issues
            for page_idx in 0..num_pages {
                let page_vaddr = vaddr as usize + page_idx * PAGE_SIZE;
                let page_offset = page_idx * PAGE_SIZE;

                // Get the physical page from the VMO
                let page_entry = vmo_pages.get(&page_offset);

                let paddr = match page_entry {
                    Some(entry) => {
                        if !entry.present {
                            // Restore interrupts before returning error
                            if interrupt_flags & (1 << 9) != 0 {
                                unsafe { crate::arch::amd64::init::arch_enable_ints(); }
                            }
                            return Err("VMO page not present");
                        }
                        entry.paddr
                    }
                    None => {
                        // Restore interrupts before returning error
                        if interrupt_flags & (1 << 9) != 0 {
                            unsafe { crate::arch::amd64::init::arch_enable_ints(); }
                        }
                        return Err("VMO page not present");
                    }
                };

                // Map the page directly - we need to re-enable interrupts temporarily
                // for map_page since it may need to allocate page tables
                if interrupt_flags & (1 << 9) != 0 {
                    unsafe { crate::arch::amd64::init::arch_enable_ints(); }
                }

                self.map_page(page_vaddr as u64, paddr, flags)?;

                // Re-disable interrupts for the next iteration
                let _ = unsafe { crate::arch::amd64::init::arch_disable_ints() };
            }
        } // Lock is released here

        // Restore interrupt state (only re-enable if they were enabled before)
        if interrupt_flags & (1 << 9) != 0 {
            unsafe { crate::arch::amd64::init::arch_enable_ints(); }
        }

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

    /// Map a single page
    ///
    /// # Arguments
    ///
    /// * `vaddr` - Virtual address (must be page-aligned)
    /// * `paddr` - Physical address (must be page-aligned)
    /// * `flags` - Page flags (PF_R, PF_W, PF_X)
    fn map_page(&self, vaddr: u64, paddr: PAddr, flags: u32) -> Result<(), &'static str> {
        // Helper: get virtual address of a page table from a PML4/PDP/PD/PT entry
        // CRITICAL: Always call this AFTER updating the parent entry, never cache and reuse!
        unsafe fn table_from_entry(entry: u64) -> *mut pt_entry_t {
            let paddr = entry & !0xFFF;
            crate::mm::pmm::paddr_to_vaddr(paddr) as *mut pt_entry_t
        }

        unsafe {
            let pml4 = self.page_table.virt;

            // Walk the page tables
            let pml4_idx = pml4_index(vaddr as usize);
            let pdp_idx = pdp_index(vaddr as usize);
            let pd_idx = pd_index(vaddr as usize);
            let pt_idx = pt_index(vaddr as usize);

            // Get or create PDP entry - NEVER reuse kernel entries for userspace
            if (*pml4.add(pml4_idx) & 1) == 0 {
                // Allocate new PDP for userspace mapping
                let new_pdp = self.alloc_page_table()?;
                *pml4.add(pml4_idx) = (new_pdp | 7); // Present + Writable + User
            } else {
                // Entry exists - check if it's a kernel entry (no USER bit)
                let existing = *pml4.add(pml4_idx);
                if existing & 4 == 0 {
                    // KERNEL-OWNED PDP - Copy kernel entries with USER bit added
                    let new_pdp = self.alloc_page_table()?;
                    let old_pdp = table_from_entry(existing);
                    let new_pdp_vaddr = table_from_entry(new_pdp);
                    // Copy all entries, adding USER bit to present entries
                    for i in 0..512 {
                        let entry = *old_pdp.add(i);
                        *new_pdp_vaddr.add(i) = if entry & 1 != 0 { entry | 4 } else { 0 };
                    }
                    *pml4.add(pml4_idx) = (new_pdp | 7);
                }
            }

            // CRITICAL: Re-read PML4 entry after potential update
            let pdp = table_from_entry(*pml4.add(pml4_idx));

            // Get or create PD entry - NEVER reuse kernel entries for userspace
            if (*pdp.add(pdp_idx) & 1) == 0 {
                // Allocate new PD for userspace mapping
                let new_pd = self.alloc_page_table()?;
                *pdp.add(pdp_idx) = (new_pd | 7);
            } else {
                // Entry exists - check if it's a kernel entry (no USER bit)
                let existing = *pdp.add(pdp_idx);
                if existing & 4 == 0 {
                    // KERNEL-OWNED PD - Copy kernel entries with USER bit added
                    let new_pd = self.alloc_page_table()?;
                    let old_pd = table_from_entry(existing);
                    let new_pd_vaddr = table_from_entry(new_pd);
                    // Copy all entries, adding USER bit to present entries
                    for i in 0..512 {
                        let entry = *old_pd.add(i);
                        *new_pd_vaddr.add(i) = if entry & 1 != 0 { entry | 4 } else { 0 };
                    }
                    *pdp.add(pdp_idx) = (new_pd | 7);
                }
            }

            // CRITICAL: Re-read PDP entry after potential update
            let pd = table_from_entry(*pdp.add(pdp_idx));

            // Get or create PT entry - NEVER reuse kernel entries for userspace
            if (*pd.add(pd_idx) & 1) == 0 {
                // Allocate new PT for userspace mapping
                let new_pt = self.alloc_page_table()?;
                *pd.add(pd_idx) = (new_pt | 7);
            } else {
                // Entry exists - check if it's a kernel entry (no USER bit)
                let existing = *pd.add(pd_idx);
                if existing & 4 == 0 {
                    // KERNEL-OWNED PT - Copy kernel entries with USER bit added
                    let new_pt = self.alloc_page_table()?;
                    let old_pt = table_from_entry(existing);
                    let new_pt_vaddr = table_from_entry(new_pt);
                    // Copy all entries, adding USER bit to present entries
                    for i in 0..512 {
                        let entry = *old_pt.add(i);
                        *new_pt_vaddr.add(i) = if entry & 1 != 0 { entry | 4 } else { 0 };
                    }
                    *pd.add(pd_idx) = (new_pt | 7);
                }
            }

            // CRITICAL: Re-read PD entry after potential update
            let pt = table_from_entry(*pd.add(pd_idx));

            // Set the final page table entry
            let mut pt_entry = paddr | 1; // Present

            if flags & 0x2 != 0 {
                // PF_W - Writable
                pt_entry |= 2;
            }

            if flags & 0x1 == 0 {
                // Not PF_R? Actually for x86, all pages are readable
                // The user bit is set separately
            }

            // Set user bit (CPL=3 can access)
            pt_entry |= 4;

            *pt.add(pt_idx) = pt_entry;

            // Debug: map_page complete
            {
                let msg = b"[MAP_PAGE] Page mapped successfully\n";
                for &b in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
            }

            Ok(())
        }
    }

    /// Allocate a new page table
    ///
    /// # Returns
    ///
    /// Physical address of the new page table
    fn alloc_page_table(&self) -> Result<PAddr, &'static str> {
        use crate::mm::pmm;

        let paddr = pmm::pmm_alloc_kernel_page()
            .map_err(|_| "Failed to allocate page table")?;

        let vaddr = pmm::paddr_to_vaddr(paddr) as *mut u8;

        unsafe {
            // Zero the page using volatile writes
            // Using volatile ensures the writes are not optimized away
            let ptr = vaddr as *mut u64;
            for i in 0..(PAGE_SIZE / 8) {
                ptr.add(i).write_volatile(0);
            }
        }

        Ok(paddr)
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
