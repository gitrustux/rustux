// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Minimal userspace test program for mexec transition
//!
//! This is the simplest possible userspace program to test
//! that the kernel→userspace transition works correctly.
//!
//! It prints "Hello from userspace!" to the debug console (port 0xE9)
//! and then spins forever.

#![no_std]
#![no_main]

use core::arch::asm;

/// Debug console port (QEMU debugcon)
const DEBUG_PORT: u16 = 0xE9;

/// Write a byte to the debug console
#[inline(always)]
unsafe fn debug_write_byte(b: u8) {
    asm!(
        "out dx, al",
        in("dx") DEBUG_PORT,
        in("al") b,
        options(nomem, nostack)
    );
}

/// Write a string to the debug console
unsafe fn debug_write_str(s: &str) {
    for &b in s.as_bytes() {
        debug_write_byte(b);
    }
}

/// Userspace entry point
///
/// This is called by the kernel after mexec transitions to userspace.
/// The kernel should have set up:
/// - User code segment (CS)
/// - User data segment (DS, ES, SS)
/// - Stack pointer (RSP)
/// - Entry point (RIP → this function)
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // Print "Hello from userspace!\n" to debug console
        debug_write_str("Hello from userspace!\n");

        // Print our current privilege level (CS & 3)
        let mut cs: u16;
        asm!("mov {0:x}, cs", out(reg) cs);
        debug_write_str("Running at CPL ");
        let cpl = cs & 3;
        debug_write_byte(b'0' + cpl as u8);
        debug_write_str("\n");

        // Infinite loop (we can't exit yet - no syscall implementation)
        loop {
            asm!("hlt");
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        debug_write_str("USERSPACE PANIC!\n");
        loop {
            asm!("hlt");
        }
    }
}
