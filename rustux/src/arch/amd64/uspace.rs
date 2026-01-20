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
    // Step 1: Load CR3 (switch to process page tables)
    asm!(
        "mov cr3, {cr3}",
        cr3 = in(reg) cr3,
        options(nostack)
    );

    // Step 2: Set up user stack
    asm!(
        "mov rsp, {stack}",
        stack = in(reg) stack_top,
        options(nostack)
    );

    // Step 3: Set up user data segments
    asm!(
        "mov ds, {ds}",
        "mov es, {ds}",
        "mov fs, {ds}",
        "mov gs, {ds}",
        "mov ss, {ds}",
        ds = in(reg) USER_DS as u16,
        options(nostack)
    );

    // Step 4: Use IRETQ to switch to user mode
    // IRETQ pops: RIP, CS, RFLAGS
    // We push: user SS, user RSP, RFLAGS, user CS, user RIP
    asm!(
        "push {ss}",          // Stack selector
        "push {rsp_val}",     // Stack pointer
        "push {rflags}",      // RFLAGS (enable interrupts)
        "push {cs}",          // Code selector
        "push {entry}",       // Entry point
        "iretq",              // Interrupt return to user mode
        ss = in(reg) USER_DS as u64,
        rsp_val = in(reg) stack_top,
        rflags = in(reg) 0x202u64, // IF=1 (interrupts enabled)
        cs = in(reg) USER_CS,
        entry = in(reg) entry,
        options(noreturn, nostack)
    );
}
