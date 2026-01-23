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

    // Create new address space
    let address_space = AddressSpace::new()
        .map_err(|_| "Failed to create address space")?;

    // Map each segment into the address space
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

    // Pre-allocate stack pages by writing zeros
    // This allocates physical pages for the stack before mapping
    let stack_size = loaded_elf.stack_size as usize;
    let page_size = 4096;
    let num_pages = (stack_size + page_size - 1) / page_size;
    let zero_page = [0u8; 4096];

    for page_idx in 0..num_pages {
        let offset = page_idx * page_size;
        // Write a zero page to trigger PMM allocation
        let bytes_to_write = if offset + page_size <= stack_size {
            &zero_page[..]
        } else {
            // Last page might be partial
            &zero_page[..stack_size - offset]
        };
        stack_vmo.write(offset, bytes_to_write)
            .map_err(|_| "Failed to allocate stack pages")?;
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
