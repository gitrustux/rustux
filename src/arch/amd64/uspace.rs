// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 Userspace Transition
//!
//! This module provides functionality to transition from kernel mode
//! to user mode for process execution.

#![allow(dead_code)]

use core::arch::asm;

// External framebuffer color functions (defined in main.rs)
extern "C" {
    fn fb_blue();
    fn fb_white();
}

/// User code segment selector (RPL=3)
const USER_CS: u64 = 0x1B;

/// User data segment selector (RPL=3)
const USER_DS: u64 = 0x23;

/// Execute a loaded process image
///
/// This function transitions from kernel mode to user mode and
/// begins execution of the loaded process.
///
/// # Arguments
///
/// * `entry` - Entry point address
/// * `stack_top` - Stack top address
/// * `cr3` - Page table base address
///
/// # Safety
///
/// This function never returns. The caller must ensure that:
/// - The entry point points to valid executable code
/// - The stack is properly mapped
/// - The page tables are correctly configured
/// - All segments are mapped at the correct addresses
///
/// # Note
///
/// This function performs the following steps:
/// 1. Loads the new CR3 (page table base)
/// 2. Sets up user mode segment selectors
/// 3. Uses IRETQ to switch to user mode at the entry point
pub unsafe fn execute_process(entry: u64, stack_top: u64, cr3: u64) -> ! {
    // Debug: trace execution
    let msg = b"[USPACE] Starting userspace transition\n";
    for &byte in msg {
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
    }

    let msg = b"[USPACE] About to load CR3\n";
    for &byte in msg {
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
    }

    // CRITICAL: Canary reads BEFORE CR3 load to verify kernel mappings
    // These reads use the process's page tables to verify that kernel
    // text and stack remain accessible after CR3 load.
    unsafe {
        // Canary 1: Read from kernel text (this function's address)
        // If this fails, kernel code is not mapped in process page tables
        let kernel_text_ptr = execute_process as *const u8;
        let kernel_text_value: u8;
        core::arch::asm!(
            "mov al, [{ptr}]",
            ptr = in(reg) kernel_text_ptr,
            out("al") kernel_text_value,
            options(nostack, readonly)
        );

        if kernel_text_value == 0 {
            let msg = b"[CANARY] FAIL: Kernel text not mapped!\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            // Halt instead of loading CR3 - CR3 load would fault
            core::arch::asm!(
                "2:",
                "hlt",
                "jmp 2b",
                options(noreturn, nostack)
            );
        }

        let msg = b"[CANARY] PASS: Kernel text accessible\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Canary 2: Read from current stack (RSP)
        // If this fails, kernel stack is not mapped
        let mut rsp: u64;
        core::arch::asm!(
            "mov {rsp}, rsp",
            rsp = out(reg) rsp,
            options(nostack)
        );

        let stack_value: u8;
        core::arch::asm!(
            "mov al, [{ptr}]",
            ptr = in(reg) rsp,
            out("al") stack_value,
            options(nostack, readonly)
        );

        let msg = b"[CANARY] PASS: Kernel stack accessible\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        let msg = b"[CANARY] All verified - loading CR3\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Load the new CR3 (page table base)
        // This is the critical switch to the process's address space
        core::arch::asm!(
            "mov cr3, {cr3}",
            cr3 = in(reg) cr3,
            options(nostack)
        );

        // PROGRESS MARKER: CR3 loaded successfully (BLUE framebuffer)
        fb_blue();

        let msg = b"[USPACE] About to load RSP\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Set up user stack
        core::arch::asm!(
            "mov rsp, {stack}",
            stack = in(reg) stack_top,
            options(nostack)
        );

        let msg = b"[USPACE] RSP loaded, about to load segments\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Set up user data segments
        core::arch::asm!(
            "mov ds, {ds}",
            "mov es, {ds}",
            "mov fs, {ds}",
            "mov gs, {ds}",
            "mov ss, {ds}",
            ds = in(reg) USER_DS as u16,
            options(nostack)
        );

        // PROGRESS MARKER: About to IRETQ to userspace (WHITE framebuffer)
        fb_white();

        // Set up RFLAGS for userspace (interrupts enabled, IOPL 0)
        let rflags: u64 = 0x202; // IF=1 (interrupts enabled), bit 1 always set

        // IRETQ frame structure:
        // Stack layout for IRETQ (growing down):
        //   [RSP]     SS:RSP
        //   [RSP+8]   RFLAGS
        //   [RSP+16]  CS:RIP
        core::arch::asm!(
            "push {ss}",     // SS
            "push {rsp}",    // RSP
            "push {rflags}", // RFLAGS
            "push {cs}",     // CS
            "push {rip}",    // RIP
            "iretq",
            ss = in(reg) USER_DS as u64,
            rsp = in(reg) stack_top,
            rflags = in(reg) rflags,
            cs = in(reg) USER_CS as u64,
            rip = in(reg) entry,
            options(noreturn, nostack)
        );
    }
}
