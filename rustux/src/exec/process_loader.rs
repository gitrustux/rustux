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
    // Debug print at the very start
    unsafe {
        let msg = b"[P-LOADER] load_elf_process called\n";
        for &byte in msg {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // Load ELF segments into VMOs
    let loaded_elf = load_elf(elf_data)?;

    unsafe {
        let msg = b"[P-LOADER] ELF loaded, segments: ";
        for &byte in msg {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
        // Print segment count
        let count = loaded_elf.segments.len();
        let digit = if count < 10 { b'0' + count as u8 } else { b'9' };
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
    }

    // Create new address space
    let address_space = AddressSpace::new()
        .map_err(|_| "Failed to create address space")?;

    unsafe {
        let msg = b"[P-LOADER] Address space created\n";
        for &byte in msg {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // Map each segment into the address space
    // Use iter() to borrow VMOs (avoiding move issues with SpinMutex)
    for (i, segment) in loaded_elf.segments.iter().enumerate() {
        unsafe {
            let msg = b"[P-LOADER] Mapping segment ";
            for &byte in msg {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }
            let digit = if i < 10 { b'0' + i as u8 } else { b'X' };
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
        }

        // Map the segment's VMO at the correct virtual address
        // Use a reference to the VMO (avoiding move issues with SpinMutex)
        address_space.map_vmo(
            &segment.vmo,
            segment.vaddr,
            segment.size,
            segment.flags,
        ).map_err(|_| "Failed to map segment")?;

        // DEBUG: Check VMO#3 after each segment mapping
        if loaded_elf.segments.len() > 2 {
            let vmo3_pages = loaded_elf.segments[2].vmo.pages.lock();
            let vmo3_entry = vmo3_pages.get(&0);
            match vmo3_entry {
                Some(e) => {
                    unsafe {
                        let msg = b"[P-LOADER] After seg";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let digit = if i < 10 { b'0' + i as u8 } else { b'X' };
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                        let msg = b": VMO#3 present=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let digit = if e.present { b'1' } else { b'0' };
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                        let msg = b"\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }
                }
                None => {
                    unsafe {
                        let msg = b"[P-LOADER] After seg";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let digit = if i < 10 { b'0' + i as u8 } else { b'X' };
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                        let msg = b": VMO#3 MISSING!\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }
                }
            }
            drop(vmo3_pages);
        }
    }

    unsafe {
        let msg = b"[P-LOADER] All segments mapped\n";
        for &byte in msg {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // Create and map the stack
    let stack_vmo = Vmo::create(loaded_elf.stack_size as usize, VmoFlags::empty)
        .map_err(|_| "Failed to create stack VMO")?;

    // Map the stack at the high address
    let stack_bottom = loaded_elf.stack_addr - loaded_elf.stack_size;
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
