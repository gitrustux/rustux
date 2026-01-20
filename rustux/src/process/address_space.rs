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
            return Err("Virtual address not page-aligned");
        }

        let num_pages = (size as usize + PAGE_SIZE - 1) / PAGE_SIZE;

        // Use fixed-size array instead of Vec to avoid heap allocation
        let mut page_mappings = [(0u64, 0u64); 256]; // Max 256 pages per mapping
        let mut mapping_count = 0usize;
        unsafe {
            let msg = b"[MAP] Disabling interrupts...\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        // Disable interrupts during mapping to prevent interference
        let interrupt_flags = unsafe { crate::arch::amd64::init::arch_disable_ints() };

        // Debug: Print interrupt flags
        unsafe {
            let msg = b"[MAP] interrupt_flags=0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = interrupt_flags;
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
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        // Debug: Interrupts disabled
        unsafe {
            let msg = b"[MAP] Interrupts disabled\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        {
            // Lock the VMO's pages
            let vmo_pages = vmo.pages.lock();

            // Debug: Confirm lock acquired
            unsafe {
                let msg = b"[MAP] VMO pages locked\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            // Debug: Before page iteration
            unsafe {
                let msg = b"[MAP] Starting page iteration\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            // Map each page - CRITICAL SECTION, minimal debug output
            for page_idx in 0..num_pages {
                // Debug: Loop iteration start
                unsafe {
                    let msg = b"[MAP] Loop iter\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                let page_vaddr = vaddr as usize + page_idx * PAGE_SIZE;
                let page_offset = page_idx * PAGE_SIZE;

                // Debug: Print key being looked up
                unsafe {
                    let msg = b"[MAP] lookup key=";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                    let mut n = page_offset;
                    let mut buf = [0u8; 16];
                    let mut i = 0;
                    loop {
                        buf[i] = b'0' + (n % 10) as u8;
                        n /= 10;
                        i += 1;
                        if n == 0 { break; }
                    }
                    while i > 0 {
                        i -= 1;
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                    }
                    let msg = b"\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                // Debug: Before vmo_pages.get()
                unsafe {
                    let msg = b"[MAP] Before get\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                // Get the physical page from the VMO
                let page_entry = vmo_pages.get(&page_offset);

                // Debug: After vmo_pages.get()
                unsafe {
                    let msg = b"[MAP] After get\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                // Debug: Before touching page_entry
                unsafe {
                    let msg = b"[MAP] Checking page_entry\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                let paddr = match page_entry {
                    Some(entry) => {
                        // Debug: Inside Some branch
                        unsafe {
                            let msg = b"[MAP] Entry is Some\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        // Debug: Before accessing entry.present
                        unsafe {
                            let msg = b"[MAP] Checking present\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        if !entry.present {
                            unsafe {
                                let msg = b"[MAP] Not present\n";
                                for &byte in msg {
                                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                                }
                            }
                            // Restore interrupts before returning error
                            if interrupt_flags & (1 << 9) != 0 {
                                unsafe { crate::arch::amd64::init::arch_enable_ints(); }
                            }
                            return Err("VMO page not present");
                        }

                        // Debug: Before accessing entry.paddr
                        unsafe {
                            let msg = b"[MAP] Reading paddr\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        let p = entry.paddr;

                        // Debug: After reading entry.paddr
                        unsafe {
                            let msg = b"[MAP] Got paddr from entry\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        p
                    }
                    None => {
                        unsafe {
                            let msg = b"[MAP] Entry is None\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                        // Restore interrupts before returning error
                        if interrupt_flags & (1 << 9) != 0 {
                            unsafe { crate::arch::amd64::init::arch_enable_ints(); }
                        }
                        return Err("VMO page not present");
                    }
                };

                // Debug: After paddr extraction
                unsafe {
                    let msg = b"[MAP] Got paddr\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                page_mappings[mapping_count] = (page_vaddr as u64, paddr);

                // Debug: After page_mappings assignment
                unsafe {
                    let msg = b"[MAP] Stored mapping\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                mapping_count += 1;

                // Debug: After mapping_count increment
                unsafe {
                    let msg = b"[MAP] Inc mapping_count\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
            }
        } // Lock is released here

        // Restore interrupt state (only re-enable if they were enabled before)
        if interrupt_flags & (1 << 9) != 0 {
            unsafe { crate::arch::amd64::init::arch_enable_ints(); }
        }

        // Now create the page table mappings
        for i in 0..mapping_count {
            let (page_vaddr, paddr) = page_mappings[i];
            self.map_page(page_vaddr, paddr, flags)?;
        }

        // Store the mapping (clone the VMO since we only have a reference)
        // Note: This creates a new VMO with copied page data, which is what we want
        let vmo_clone = vmo.clone().map_err(|_| "Failed to clone VMO for mapping")?;
        let mapping = VmoMapping {
            vmo: vmo_clone,
            vaddr,
            size,
            flags,
        };
        self.mappings.lock().insert(vaddr, mapping);

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
        unsafe {
            let msg = b"[MAP] map_page vaddr=0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
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

            let msg = b" paddr=0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = paddr;
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
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }

            let pml4 = self.page_table.virt;

            // Walk the page tables
            let pml4_idx = pml4_index(vaddr as usize);
            let pdp_idx = pdp_index(vaddr as usize);
            let pd_idx = pd_index(vaddr as usize);
            let pt_idx = pt_index(vaddr as usize);

            // Get or create PDP entry
            let msg = b"[MAP] Checking PDP...\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let pdp_paddr = if (*pml4.add(pml4_idx) & 1) == 0 {
                // Allocate new PDP
                let new_pdp = self.alloc_page_table()?;
                *pml4.add(pml4_idx) = (new_pdp | 3); // Present + Writable
                new_pdp
            } else {
                (*pml4.add(pml4_idx)) & !0xFFF
            };

            let pdp = crate::mm::pmm::paddr_to_vaddr(pdp_paddr) as *mut pt_entry_t;

            unsafe {
                let msg = b"[MAP] Checking PD...\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            // Get or create PD entry
            let pd_paddr = if (*pdp.add(pdp_idx) & 1) == 0 {
                let new_pd = self.alloc_page_table()?;
                *pdp.add(pdp_idx) = (new_pd | 3);
                new_pd
            } else {
                unsafe {
                    let msg = b"[MAP] PD exists, reusing\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
                (*pdp.add(pdp_idx)) & !0xFFF
            };

            let pd = crate::mm::pmm::paddr_to_vaddr(pd_paddr) as *mut pt_entry_t;

            unsafe {
                let msg = b"[MAP] Checking PT...\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            // Get or create PT entry
            let pt_paddr = if (*pd.add(pd_idx) & 1) == 0 {
                let new_pt = self.alloc_page_table()?;
                *pd.add(pd_idx) = (new_pt | 3);
                new_pt
            } else {
                (*pd.add(pd_idx)) & !0xFFF
            };

            let pt = crate::mm::pmm::paddr_to_vaddr(pt_paddr) as *mut pt_entry_t;

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

        unsafe {
            let msg = b"[PT] Allocating page table...\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        let paddr = pmm::pmm_alloc_kernel_page()
            .map_err(|_| {
                unsafe {
                    let msg = b"[PT] PMM allocation failed\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
                "Failed to allocate page table"
            })?;

        unsafe {
            let msg = b"[PT] Allocated at 0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = paddr;
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
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        let vaddr = pmm::paddr_to_vaddr(paddr) as *mut u8;

        unsafe {
            let msg = b"[PT] Zeroing page at vaddr=0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = vaddr as u64;
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
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }

            // Zero the page using volatile writes
            // Using volatile ensures the writes are not optimized away
            let ptr = vaddr as *mut u64;
            for i in 0..(PAGE_SIZE / 8) {
                ptr.add(i).write_volatile(0);
            }

            let msg = b"[PT] Page zeroed\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
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
