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

/// mexec: mini-exec for kernel→userspace transition
///
/// This function transitions from kernel mode to userspace mode by:
/// 1. Disabling interrupts
/// 2. Turning off PGE (Page Global Enable) in CR4
/// 3. Creating and loading a temporary GDT
/// 4. Switching to userspace segments
/// 5. Jumping to userspace entry point
///
/// # Arguments
///
/// * `arg1` - First argument to userspace (passed in rdi)
/// * `arg2` - Second argument to userspace (passed in rsi)
/// * `user_stack` - User stack pointer (passed in rdx)
/// * `user_entry` - User program counter/entry point (passed in rcx)
/// * `aux` - Auxiliary value (passed in r8, e.g., bootimage address)
///
/// # Safety
///
/// This function never returns to the caller.
/// All arguments must be valid user space addresses.
///
/// # Assembly Calling Convention
///
/// On entry, the function receives arguments in registers:
/// - rdi = arg1
/// - rsi = arg2
/// - rdx = user_stack
/// - rcx = user_entry
/// - r8  = aux
unsafe #[naked]
pub unsafe extern "sysv64" fn mexec_asm(
    arg1: u64,
    arg2: u64,
    user_stack: u64,
    user_entry: u64,
    aux: u64,
) -> ! {
    naked_asm!(
        // Disable interrupts
        "cli",

        // Turn off PGE (Page Global Enable) in CR4
        "mov r11, cr4",
        "and r11, ~0x80",
        "mov cr4, r11",

        // Load GDT pointer
        "lea r11, [rip + 2f]",    // 2f = forward label 2
        "lgdt [rip + 1f]",        // 1f = forward label 1

        // Switch to user data segment
        "mov r11, 0x23",
        "mov ds, r11w",
        "mov es, r11w",
        "mov ss, r11w",

        // Far jump to user code segment
        "lea r11, [rip + 0f]",    // 0f = forward label 0
        "pushq 0x18",             // User code selector (index 3)
        "push r11",
        "lretq",

        "0:",  // New CS

        // Set up user stack
        "mov rsp, rdx",

        // Clear registers
        "xor rbp, rbp",
        "xor rbx, rbx",

        // Jump to userspace entry point
        "jmp rcx",

        // Crash if we get here
        "ud2",

        // GDT data
        ".align 16",
        "2:",  // mexec_gdt
        ".quad 0",               // Null entry
        ".quad 0x00AF9B000000FFFF",  // Kernel 64-bit code
        ".quad 0x00CF93000000FFFF",  // Kernel data
        ".quad 0x00AFFB000000FFFF",  // User 64-bit code
        "2e:",  // mexec_gdt_end

        ".align 8",
        "1:",  // mexec_gdt_pointer
        ".short 2e - 2 - 1",     // GDT limit
        ".quad 0",               // GDT base (filled at runtime)
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
