// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Process Loader
//!
//! This module provides functionality to load ELF binaries into
//! new process address spaces and prepare them for execution.

#![allow(dead_code)]

use crate::exec::elf::{load_elf, LoadedElf};
use crate::process::AddressSpace;
use crate::object::{Vmo, VmoFlags};
use crate::mm::pmm;

use crate::arch::amd64::mm::page_tables::pt_entry_t;

// Page size
const PAGE_SIZE: usize = 4096;

// Page table indices from virtual address
fn pml4_index(vaddr: usize) -> usize {
    (vaddr >> 39) & 0x1FF
}

fn pdp_index(vaddr: usize) -> usize {
    (vaddr >> 30) & 0x1FF
}

fn pd_index(vaddr: usize) -> usize {
    (vaddr >> 21) & 0x1FF
}

fn pt_index(vaddr: usize) -> usize {
    (vaddr >> 12) & 0x1FF
}

/// Allocate a new page table for kernel page table mapping
/// Returns the physical address of the allocated page
fn alloc_page_table_for_kernel() -> Result<u64, &'static str> {
    let paddr = pmm::pmm_alloc_kernel_page()
        .map_err(|_| "Failed to allocate page table")?;

    unsafe {
        let msg = b"[PT_ALLOC] Page allocated, zeroing...\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    let vaddr = pmm::paddr_to_vaddr(paddr) as *mut u8;

    unsafe {
        // Zero the page using volatile writes
        let ptr = vaddr as *mut u64;
        for i in 0..(PAGE_SIZE / 8) {
            ptr.add(i).write_volatile(0);
        }
    }

    unsafe {
        let msg = b"[PT_ALLOC] Zeroing complete\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    Ok(paddr)
}

/// Map a page into a specific page table (kernel's PML4)
fn map_page_into_kernel_table(
    kernel_pml4_vaddr: *mut pt_entry_t,
    vaddr: u64,
    paddr: u64,
    flags: u32,
) -> Result<(), &'static str> {
    unsafe {
        // Walk the page tables
        let pml4_idx = pml4_index(vaddr as usize);
        let pdp_idx = pdp_index(vaddr as usize);
        let pd_idx = pd_index(vaddr as usize);
        let pt_idx = pt_index(vaddr as usize);

        // Get or create PDP entry
        let pdp_paddr = if (*kernel_pml4_vaddr.add(pml4_idx) & 1) == 0 {
            // Allocate new PDP - returns physical address directly
            let new_pdp_paddr = alloc_page_table_for_kernel()?;
            let msg = b"[MAP_P] Writing PML4 entry\n";
            for &b in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
            }
            *kernel_pml4_vaddr.add(pml4_idx) = (new_pdp_paddr | 3); // Present + Writable
            let msg = b"[MAP_P] PML4 entry written\n";
            for &b in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
            }
            new_pdp_paddr
        } else {
            (*kernel_pml4_vaddr.add(pml4_idx)) & !0xFFF
        };

        let pdp = pmm::paddr_to_vaddr(pdp_paddr) as *mut pt_entry_t;

        let msg = b"[MAP_P] About to read PDP\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }

        // Get or create PD entry
        let pd_paddr = if (*pdp.add(pdp_idx) & 1) == 0 {
            let new_pd_paddr = alloc_page_table_for_kernel()?;
            *pdp.add(pdp_idx) = (new_pd_paddr | 3);
            new_pd_paddr
        } else {
            (*pdp.add(pdp_idx)) & !0xFFF
        };

        let pd = pmm::paddr_to_vaddr(pd_paddr) as *mut pt_entry_t;

        // Get or create PT entry
        let pt_paddr = if (*pd.add(pd_idx) & 1) == 0 {
            let new_pt_paddr = alloc_page_table_for_kernel()?;
            *pd.add(pd_idx) = (new_pt_paddr | 3);
            new_pt_paddr
        } else {
            (*pd.add(pd_idx)) & !0xFFF
        };

        let pt = pmm::paddr_to_vaddr(pt_paddr) as *mut pt_entry_t;

        // Set the final page table entry
        let mut pt_entry = paddr | 1; // Present

        if flags & 0x2 != 0 {
            // PF_W - Writable
            pt_entry |= 2;
        }

        // Set user bit (CPL=3 can access)
        pt_entry |= 4;

        *pt.add(pt_idx) = pt_entry;

        Ok(())
    }
}

/// Map a VMO into the kernel page table
fn map_vmo_into_kernel_table(
    kernel_pml4_vaddr: *mut pt_entry_t,
    vmo: &Vmo,
    vaddr: u64,
    size: u64,
    flags: u32,
) -> Result<(), &'static str> {
    let num_pages = (size as usize + PAGE_SIZE - 1) / PAGE_SIZE;

    unsafe {
        let msg = b"[MAP_K] Mapping ";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = num_pages;
        let mut buf = [0u8; 16];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b" pages...\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    let vmo_pages = vmo.pages.lock();

    for page_idx in 0..num_pages {
        // Print progress every 16 pages
        if page_idx % 16 == 0 {
            unsafe {
                let msg = b".";
                for &b in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
            }
        }

        let page_vaddr = vaddr as usize + page_idx * PAGE_SIZE;
        let page_offset = page_idx * PAGE_SIZE;

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

        match map_page_into_kernel_table(kernel_pml4_vaddr, page_vaddr as u64, paddr, flags) {
            Ok(_) => {}
            Err(e) => {
                unsafe {
                    let msg = b"\n[MAP_K] Error at page ";
                    for &b in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                    }
                    let mut n = page_idx;
                    let mut buf = [0u8; 16];
                    let mut i = 0;
                    if n == 0 {
                        buf[i] = b'0';
                        i += 1;
                    } else {
                        while n > 0 {
                            let digit = (n & 0xF) as u8;
                            buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                            n >>= 4;
                            i += 1;
                        }
                    }
                    while i > 0 {
                        i -= 1;
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                    }
                    let msg = b": ";
                    for &b in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                    }
                    for b in (*e).bytes() {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                    }
                    let msg = b"\n";
                    for &b in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                    }
                }
                return Err(e);
            }
        };
    }

    unsafe {
        let msg = b"\n[MAP_K] Done\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    Ok(())
}

/// Information needed to start execution of a loaded process
pub struct ProcessImage {
    /// Entry point address
    pub entry: u64,
    /// Address space for the process
    pub address_space: AddressSpace,
    /// Stack top address
    pub stack_top: u64,
    /// Stack size
    pub stack_size: u64,
}

/// Load an ELF binary into a new process
///
/// This function:
/// 1. Parses and loads the ELF binary
/// 2. Creates a new address space
/// 3. Maps all ELF segments into the address space
/// 4. Creates and maps a user stack
/// 5. Returns information needed to start execution
///
/// # Arguments
///
/// * `elf_data` - Raw ELF file contents
///
/// # Returns
///
/// * `Ok(ProcessImage)` - Loaded process ready to execute
/// * `Err(&str)` - Loading failed
pub fn load_elf_process(elf_data: &[u8]) -> Result<ProcessImage, &'static str> {
    // Load ELF segments into VMOs
    let loaded_elf = load_elf(elf_data)?;

    // Create new address space (for future CR3 switching)
    let address_space = AddressSpace::new()
        .map_err(|_| "Failed to create address space")?;

    // DISABLED: Cannot modify kernel's PML4 (it's read-only or not mapped)
    // TODO: Need to set up recursive mapping or use different approach
    // The kernel PML4 is created by UEFI and is not writable by the kernel.
    // We need to either:
    // 1. Set up a recursive mapping in the kernel's PML4
    // 2. Map the PML4 page itself into the kernel address space
    // 3. Use a different approach (e.g., create new page tables for processes)
    //
    // For now, we skip kernel page table mapping and use process PML4 instead.
    // This requires proper CR3 switching when executing userspace code.

    // The following code is disabled because it tries to modify the kernel's PML4:
    // let kernel_cr3 = init::x86_read_cr3();
    // let kernel_pml4_paddr = kernel_cr3 & !0xFFF;
    // let kernel_pml4_vaddr = crate::mm::pmm::paddr_to_vaddr(kernel_pml4_paddr) as *mut crate::arch::amd64::mm::page_tables::pt_entry_t;
    // ... (map segments into kernel PML4)
    // ... (map stack into kernel PML4)

    // Map each segment into the process address space (for future use)
    for segment in loaded_elf.segments.iter() {
        unsafe {
            let msg = b"[MAP] About to map segment at vaddr: 0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = segment.vaddr;
            let mut buf = [0u8; 16];
            let mut i = 0;
            if n == 0 {
                buf[i] = b'0';
                i += 1;
            } else {
                while n > 0 {
                    let digit = (n & 0xF) as u8;
                    buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                    n >>= 4;
                    i += 1;
                }
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
        // Map into process page table (not kernel's PML4)
        address_space.map_vmo(
            &segment.vmo,
            segment.vaddr,
            segment.size,
            segment.flags,
        )?;
    }

    // Create and map the stack
    let stack_vmo = Vmo::create(loaded_elf.stack_size as usize, VmoFlags::empty)
        .map_err(|_| "Failed to create stack VMO")?;

    // CRITICAL: Allocate physical pages for the stack by zeroing it out
    // VMO::create() only sets up the structure - pages are allocated on write
    // We need to pre-allocate all stack pages before mapping
    // Write one page at a time for efficiency (matches VMO page allocation)
    const PAGE_SIZE: usize = 4096;
    let zero_page = [0u8; PAGE_SIZE];
    let mut offset = 0;
    while offset < loaded_elf.stack_size as usize {
        let chunk_size = core::cmp::min(PAGE_SIZE, loaded_elf.stack_size as usize - offset);
        stack_vmo.write(offset, &zero_page[..chunk_size])
            .map_err(|_| "Failed to allocate stack pages")?;
        offset += chunk_size;
    }

    // Map the stack at the high address
    // Ensure stack_bottom is page-aligned (round down to nearest 4KB)
    let stack_bottom = (loaded_elf.stack_addr - loaded_elf.stack_size) & !0xFFF;
    address_space.map_vmo(
        &stack_vmo,
        stack_bottom,
        loaded_elf.stack_size,
        0x6, // PF_R | PF_W (readable + writable)
    ).map_err(|_| "Failed to map stack")?;

    Ok(ProcessImage {
        entry: loaded_elf.entry,
        address_space,
        stack_top: loaded_elf.stack_addr,
        stack_size: loaded_elf.stack_size,
    })
}
