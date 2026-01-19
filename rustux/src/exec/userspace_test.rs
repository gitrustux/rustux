// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Userspace Execution Test
//!
//! This module provides a simple test for the mexec (kernel to userspace
//! transition) functionality. It embeds a minimal userspace binary
//! and provides a function to execute it.

/// Embedded userspace test binary
///
/// This is a minimal x86_64 ELF binary that:
/// 1. Prints "Hello from userspace!" to the debug console (port 0xE9)
/// 2. Displays the current privilege level (CPL)
/// 3. Spins forever (no syscall support yet)
///
/// The binary was compiled from userspace/test/src/main.rs with:
/// - Entry point: 0x10005a (code loads at 0x100000)
/// - Stack: 1MB at 0x800000
///
/// To rebuild the userspace test:
/// ```bash
/// cd userspace/test
/// cargo build --release --target x86_64-unknown-none
/// objcopy -O binary target/x86_64-unknown-none/release/rustux-userspace-test userspace-test.bin
/// cp userspace-test.bin ../../src/exec/userspace-test.bin
/// ```
///
/// Then embed the binary here using include_bytes!.
#[cfg(feature = "userspace_test")]
#[allow(dead_code)]
static USERSPACE_BIN: &[u8] = include_bytes!("userspace-test.bin");

/// Userspace entry point (from ELF header)
///
/// This is the entry point address from the userspace test ELF.
/// It was extracted using: readelf -h userspace-test.bin
const USERSPACE_ENTRY: u64 = 0x100000;

/// Userspace stack address (from linker script)
///
/// The userspace binary expects its stack at this address.
const USERSPACE_STACK: u64 = 0x800000 + 0x100000; // Top of 1MB stack region

/// Execute the embedded userspace test program
///
/// This function:
/// 1. Verifies the userspace binary is embedded
/// 2. Uses mexec to transition to userspace
/// 3. Never returns (jumps to userspace)
///
/// # Safety
///
/// This function never returns. The caller must ensure that:
/// - The kernel is properly initialized
/// - Page tables are configured for userspace access
/// - The userspace binary is valid
///
/// # Arguments
///
/// * `enable_mexec` - If false, prints message but doesn't execute mexec
#[cfg(feature = "userspace_test")]
pub unsafe fn execute_userspace_test(enable_mexec: bool) {
    extern crate alloc;

    use alloc::format;
    use crate::arch::amd64::mexec;

    // Print debug message to debug console port (0xE9)
    let msg = b"[KERNEL] Attempting userspace transition...\n";
    for &byte in msg {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    if !enable_mexec {
        let skip_msg = b"[KERNEL] mexec disabled, halting...\n";
        for &byte in skip_msg {
            unsafe {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }
        }
        loop {
            core::hint::spin_loop();
        }
    }

    // Verify binary is embedded
    if USERSPACE_BIN.is_empty() {
        let err_msg = b"[KERNEL] ERROR: Userspace binary not embedded!\n";
        for &byte in err_msg {
            unsafe {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }
        }
        loop {
            core::hint::spin_loop();
        }
    }

    let info_msg = b"[KERNEL] Userspace binary loaded, jumping to userspace...\n";
    for &byte in info_msg {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // Execute mexec to transition to userspace
    // Note: This will jump to the raw binary at the embedded address
    // The binary must be loaded at the correct address for this to work
    mexec::jump_to_userspace(USERSPACE_ENTRY, USERSPACE_STACK);
}

/// Stub function when userspace_test feature is not enabled
#[cfg(not(feature = "userspace_test"))]
pub unsafe fn execute_userspace_test(_enable_mexec: bool) {
    let msg = b"[KERNEL] Userspace test not enabled (build with --features userspace_test)\n";
    for &byte in msg {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }
}

/// Simple test of mexec without actual userspace binary
///
/// This function tests the mexec transition with a minimal setup.
/// It's useful for verifying that the transition mechanism works.
pub unsafe fn test_mexec_minimal() {
    use crate::arch::amd64::mexec;

    let msg = b"[KERNEL] Testing minimal mexec transition...\n";
    for &byte in msg {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") 0xE9u16,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // For testing, we'll jump to a known address
    // In reality, this would cause a page fault or execute garbage
    // But it tests the transition mechanism itself
    let test_entry = 0x100000u64;
    let test_stack = 0x800000u64;

    mexec::jump_to_userspace(test_entry, test_stack);
}
