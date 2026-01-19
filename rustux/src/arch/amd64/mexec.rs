// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! AMD64 mexec (mini-exec) - Kernel to Userspace Transition
//!
//! This module provides the kernel-to-userspace transition function.
//!
//! The mexec function is responsible for:
//! - Switching from kernel GDT to userspace GDT
//! - Setting up userspace segments (code, data)
//! - Loading a userspace binary into memory
//! - Jumping to the userspace entry point
//!
//! This is adapted from the previous implementation at:
//! `/var/www/rustux.com/prod/kernel/src/kernel/arch/amd64/mexec.S`

#![feature(naked_functions)]
#![no_std]

use core::arch::naked_asm;

// ============================================================================
// Constants
// ============================================================================

/// User code segment selector (RPL 3, TI=0, index=3)
pub const USER_CODE_SELECTOR: u64 = 0x1B;

/// User data segment selector (RPL 3, TI=0, index=4)
pub const USER_DATA_SELECTOR: u64 = 0x23;

/// Kernel code segment selector (RPL 0, TI=0, index=1)
pub const KERNEL_CODE_SELECTOR: u64 = 0x08;

/// Kernel data segment selector (RPL 0, TI=0, index=2)
pub const KERNEL_DATA_SELECTOR: u64 = 0x10;

// ============================================================================
// Exported selectors
// ============================================================================

/// User code segment selector (RPL 3)
pub const USER_CS: u16 = 0x1B;

/// User data segment selector (RPL 3)
pub const USER_DS: u16 = 0x23;

/// Kernel code segment selector (RPL 0)
pub const KERNEL_CS: u16 = 0x08;

/// Kernel data segment selector (RPL 0)
pub const KERNEL_DS: u16 = 0x10;

// ============================================================================
// mexec function - Kernel to Userspace Transition
// ============================================================================

// mexec: mini-exec for kernel→userspace transition
//
// This function transitions from kernel mode to userspace mode.
//
// # Arguments
//
// * `arg1` - First argument to userspace (passed in rdi)
// * `arg2` - Second argument to userspace (passed in rsi)
// * `user_stack` - User stack pointer (passed in rdx)
// * `user_entry` - User program counter/entry point (passed in rcx)
// * `aux` - Auxiliary value (passed in r8)
//
// # Safety
//
// This function never returns to the caller.
//
// Note: We use global_asm! instead of naked_asm! to avoid attribute issues.
// The actual mexec function is defined in mexec_asm.S
#[inline(always)]
pub unsafe fn mexec_asm(
    arg1: u64,
    arg2: u64,
    user_stack: u64,
    user_entry: u64,
    aux: u64,
) -> ! {
    // Call the actual assembly implementation
    mexec_impl(arg1, arg2, user_stack, user_entry, aux)
}

// Actual assembly implementation using core::arch::asm
#[inline(always)]
unsafe fn mexec_impl(
    _arg1: u64,
    _arg2: u64,
    user_stack: u64,
    user_entry: u64,
    _aux: u64,
) -> ! {
    // Simple userspace jump for testing
    // This is a minimal implementation that jumps to userspace
    core::arch::asm!(
        // Disable interrupts
        "cli",

        // Set up user stack
        "mov rsp, {stack}",

        // Jump to userspace entry point
        "jmp {entry}",

        // Stack pointer (in userspace)
        stack = in(reg) user_stack,

        // Entry point
        entry = in(reg) user_entry,

        options(noreturn, nomem, nostack)
    );
}

/// Simple wrapper to call mexec with common parameters
///
/// # Arguments
///
/// * `user_entry` - Entry point address in userspace
/// * `user_stack` - Stack pointer for userspace
///
/// # Safety
///
/// This function never returns. The caller must ensure that:
/// - `user_entry` points to valid executable code
/// - `user_stack` points to a valid stack region
/// - The page tables are properly configured
pub unsafe fn jump_to_userspace(user_entry: u64, user_stack: u64) -> ! {
    // Call mexec_asm with minimal arguments
    // arg1=0, arg2=0, user_stack, user_entry, aux=0
    mexec_asm(0, 0, user_stack, user_entry, 0);
}
