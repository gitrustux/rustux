// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Test Kernel Entry Point
//!
//! This module provides a simple test entry point for the kernel
//! that can be used to verify the interrupt system works.
//!
//! This is NOT the production kernel entry point - it's only for testing.

#![no_std]

use crate::arch::amd64;

/// Test kernel entry point
///
/// This is called from the bootloader to test the interrupt system.
/// It performs the following:
/// 1. Prints a banner to QEMU debug console
/// 2. Tests the interrupt system (GDT, IDT, APIC, Timer)
/// 3. Halts if test passes, loops if it fails
///
/// # Safety
///
/// This function should only be called from bootloader code with proper setup.
#[no_mangle]
pub extern "C" fn test_kernel_main() -> ! {
    // Print banner
    qemu_print("\n");
    qemu_print("╔══════════════════════════════════════════════════════════╗\n");
    qemu_print("║           RUSTUX KERNEL - INTERRUPT TEST                 ║\n");
    qemu_print("║           Testing Migrated Boot Infrastructure           ║\n");
    qemu_print("╚══════════════════════════════════════════════════════════╝\n");
    qemu_print("\n");

    // Run the interrupt system test
    crate::arch::amd64::test::test_interrupt_system();

    // Test complete - halt
    qemu_print("\nTest complete. Halting CPU.\n");
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack));
        }
    }
}

/// Write a byte to QEMU's debug console
fn qemu_print(s: &str) {
    const QEMU_DEBUGCON_PORT: u16 = 0xE9;
    for byte in s.bytes() {
        unsafe {
            core::arch::asm!(
                "out dx, al",
                in("dx") QEMU_DEBUGCON_PORT,
                in("al") byte,
                options(nostack, nomem)
            );
        }
    }
}
