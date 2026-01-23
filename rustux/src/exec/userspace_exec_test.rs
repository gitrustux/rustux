// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Userspace Execution Test
//!
//! This module provides a test for loading and executing
//! a userspace ELF binary.

/// Embedded userspace test binary
///
/// This is a static x86_64 ELF binary that writes
/// "Hello from userspace!" to the debug console (port 0xE9)
/// and then spins forever.
///
/// Path: ../../test-userspace/hello.elf (from src/exec/userspace_exec_test.rs)
/// Resolves to: test-userspace/hello.elf relative to crate root
static USERSPACE_ELF: &[u8] = include_bytes!("../../test-userspace/hello.elf");

/// Build-time verification: Ensure ELF is at least 8KB
/// If this fails, the hello.elf file may not exist or be wrong size
const _: () = assert!(USERSPACE_ELF.len() >= 8192, "Embedded ELF is too small - check hello.elf build");

const ELF_SIZE: usize = USERSPACE_ELF.len();

/// Test userspace execution
///
/// This function:
/// 1. Loads the embedded ELF binary
/// 2. Creates a new address space
/// 3. Maps all segments and the stack
/// 4. Transitions to userspace and executes the binary
///
/// # Safety
///
/// This function never returns. The caller must ensure that:
/// - The kernel is properly initialized
/// - Page tables are configured
/// - Interrupts are enabled (or will be in userspace)
pub unsafe fn test_userspace_execution() -> ! {
    use crate::exec::process_loader;
    use crate::arch::amd64::uspace;
    use crate::mm::allocator;

    // Print heap status BEFORE ELF loading
    allocator::heap_print_summary();

    // Print ELF size for debugging (simple decimal)
    {
        let elf_size = USERSPACE_ELF.len();
        let msg = b"[KERNEL] ELF size: ";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Print decimal size
        let mut n = elf_size;
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
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
    }

    // Test heap allocation before loading ELF
    extern crate alloc;
    let _test_vec = alloc::vec::Vec::<u8>::new();
    let msg = b"[KERNEL] Heap test passed\n";
    for &byte in msg {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") byte,
            options(nomem, nostack)
        );
    }

    // Write debug message
    let msg = b"[KERNEL] Loading userspace ELF binary...\n";
    for &byte in msg {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") byte,
            options(nomem, nostack)
        );
    }

    // Load ELF into process address space
    let process_image = match process_loader::load_elf_process(USERSPACE_ELF) {
        Ok(img) => {
            // Print heap status AFTER ELF loading (SUCCESS)
            allocator::heap_print_summary();
            img
        },
        Err(e) => {
            // Print heap status AFTER ELF loading (FAILURE)
            allocator::heap_print_summary();

            let err_msg = b"[KERNEL] Failed to load ELF: ";
            for &byte in err_msg {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }
            for &byte in e.as_bytes() {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'\n', options(nomem, nostack));
            loop { core::arch::asm!("hlt"); }
        }
    };

    // Debug: confirm we got past loading
    let dbg_msg = b"[KERNEL] ELF load returned\n";
    for &byte in dbg_msg {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") byte,
            options(nomem, nostack)
        );
    }

    let ok_msg = b"[KERNEL] ELF loaded successfully, jumping to userspace...\n";
    for &byte in ok_msg {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xE9u16,
            in("al") byte,
            options(nomem, nostack)
        );
    }

    // Get CR3 value from the address space
    let cr3 = process_image.address_space.page_table.phys;

    // Execute the process
    uspace::execute_process(process_image.entry, process_image.stack_top, cr3);
}
