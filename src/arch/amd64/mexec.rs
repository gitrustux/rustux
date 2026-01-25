// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! AMD64 mexec (mini-exec) - Kernel to Userspace Transition
//!
//! This module provides the kernel-to-userspace transition function.

#![no_std]

use core::arch::asm;

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

/// Perform kernel to userspace transition
///
/// This function transitions from kernel mode to userspace mode by:
/// 1. Disabling interrupts
/// 2. Turning off PGE (Page Global Enable) in CR4
/// 3. Setting up a new GDT with user mode segments
/// 4. Switching to user data segments
/// 5. Far jumping to user code segment
/// 6. Jumping to the userspace entry point
///
/// # Arguments
///
/// * `arg1` - First argument to userspace (passed in rdi)
/// * `arg2` - Second argument to userspace (passed in rsi)
/// * `user_stack` - User stack pointer (passed in rdx)
/// * `user_entry` - User program counter/entry point (passed in rcx)
/// * `aux` - Auxiliary value (passed in r8)
///
/// # Safety
///
/// This function never returns to the caller.
/// All arguments must be valid user space addresses.
///
/// # Note on Implementation
///
/// This function is marked as `extern "sysv64"` to follow the System V AMD64 ABI,
/// but the actual transition happens via inline assembly. The `noreturn` option
/// ensures the compiler knows this function never returns.
#[inline(never)]
pub unsafe extern "sysv64" fn mexec_asm(
    arg1: u64,
    arg2: u64,
    user_stack: u64,
    user_entry: u64,
    aux: u64,
) -> ! {
    // Save userspace entry and stack for later use
    let us_entry = user_entry;
    let us_stack = user_stack;
    let us_aux = aux;

    // Step 1: Disable interrupts
    asm!("cli", options(nomem, nostack));

    // Step 2: Turn off PGE (Page Global Enable) in CR4
    // This prevents stale TLB entries from causing issues after transition
    let mut cr4: u64;
    unsafe {
        asm!(
            "mov {0}, cr4",
            out(reg) cr4,
        );
    }
    cr4 &= !0x80;
    unsafe {
        asm!(
            "mov cr4, {0}",
            in(reg) cr4,
        );
    }

    // Step 3: Create a temporary GDT with user mode segments
    // GDT layout:
    // Index 0: Null entry
    // Index 1: Kernel 64-bit code (selector 0x08)
    // Index 2: Kernel data (selector 0x10)
    // Index 3: User 64-bit code (selector 0x18) - RPL=3
    // Index 4: User data (selector 0x20) - RPL=3

    // The GDT is embedded in the code as data
    #[repr(C)]
    #[repr(align(16))]
    struct GdtEntry {
        data: [u64; 5],
    }

    // Create GDT entries
    let gdt = GdtEntry {
        data: [
            0x0000000000000000, // Index 0: Null
            0x00AF9B000000FFFF,  // Index 1: Kernel 64-bit code
            0x00CF93000000FFFF,  // Index 2: Kernel data
            0x00AFFB000000FFFF,  // Index 3: User 64-bit code (RPL=3)
            0x00CFF3000000FFFF,  // Index 4: User data (RPL=3)
        ],
    };

    // Load GDT
    let gdt_ptr = GdtPointer {
        limit: (core::mem::size_of::<GdtEntry>() - 1) as u16,
        base: &gdt as *const GdtEntry as u64,
    };

    unsafe {
        asm!(
            "lgdt [{0}]",
            in(reg) &gdt_ptr,
            options(nostack)
        );
    }

    // Step 4: Switch to user data segments (selector 0x23)
    unsafe {
        asm!(
            "mov bx, 0x23",
            "mov ds, bx",
            "mov es, bx",
            "mov ss, bx",
            options(nomem, nostack)
        );
    }

    // Step 5: Far jump to user code segment
    // We push the user CS selector (0x1B) and a return address, then use retfq
    // The retfq instruction pops both CS and RIP, switching to user mode
    unsafe {
        asm!(
            "lea r11, [2f]",       // Get address of userspace code
            "push rax",            // Align stack
            "push r11",             // Return address
            "mov rax, 0x1B",       // User code selector (RPL=3)
            "push rax",            // Push CS selector
            "retfq",               // Far return to user mode

            "2:",               // Now in userspace
            "mov rsp, {stack}", // Set user stack

            // Clear registers
            "xor rbp, rbp",
            "xor rbx, rbx",
            "xor r12, r12",
            "xor r13, r13",
            "xor r14, r14",
            "xor r15, r15",

            // Set up arguments (System V AMD64 ABI)
            "mov rdi, {arg1}",   // First argument
            "mov rsi, {arg2}",   // Second argument
            "mov rdx, {stack}",  // Third argument (stack pointer)

            // Jump to userspace entry point
            "jmp {entry}",       // Never returns

            stack = in(reg) us_stack,
            arg1 = in(reg) arg1,
            arg2 = in(reg) arg2,
            entry = in(reg) us_entry,

            options(noreturn, nomem, nostack)
        );
    }

    #[repr(C, packed)]
    struct GdtPointer {
        limit: u16,
        base: u64,
    }

    // This function never returns
    loop {}
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

/// Test userspace transition with simple debug output
///
/// This is a test function that writes to the debug console (port 0xE9)
/// before transitioning to userspace. It's used to verify that the kernel
/// can communicate with the debug console.
pub unsafe fn test_userspace_debug() {
    const DEBUG_PORT: u16 = 0xE9;
    let msg = b"[KERNEL] Attempting userspace transition...\n";

    for &byte in msg {
        unsafe {
            asm!(
                "out dx, al",
                in("dx") DEBUG_PORT,
                in("al") byte,
                options(nomem, nostack)
            );
        }
    }

    // Use a fixed address for testing
    let test_entry = 0x100000u64;
    let test_stack = 0x800000u64;

    jump_to_userspace(test_entry, test_stack);
}
