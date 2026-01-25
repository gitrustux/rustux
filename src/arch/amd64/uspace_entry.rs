// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 User Space Entry
//!
//! This module provides the function to enter user space from kernel mode.
//!
//! The entry point uses `iretq` to transition from kernel mode to user mode
//! by setting up a fake interrupt stack frame.

#![feature(naked_functions)]

use core::arch::naked_asm;

/// User data segment selector (RPL 3, TI=0, index=4)
const USER_DATA_SELECTOR: u16 = 0x23;

/// User 64-bit code segment selector (RPL 3, TI=0, index=5)
const USER_CODE_64_SELECTOR: u16 = 0x2B;

/// Default user RFLAGS value (interrupts enabled)
const DEFAULT_USER_RFLAGS: u64 = 0x202;

/// User mode segment selectors
pub mod selectors {
    /// User code selector (RPL=3, TI=0, index=5)
    pub const USER_CS: u16 = 0x2B;

    /// User data selector (RPL=3, TI=0, index=4)
    pub const USER_DS: u16 = 0x23;

    /// User code selector for 32-bit compatibility mode
    pub const USER_CS_32: u16 = 0x23;

    /// Null selector
    pub const NULL_SEL: u16 = 0;
}

/// Enter user space
///
/// This function sets up a fake interrupt stack frame and uses iretq
/// to transition from kernel mode to user mode.
///
/// # Arguments
///
/// * `arg1` - First argument to user process (in rdi)
/// * `arg2` - Second argument to user process (in rsi)
/// * `sp` - User stack pointer (in rdx)
/// * `pc` - User program counter/entry point (in rcx)
/// * `rflags` - User RFLAGS value (in r8)
///
/// # Safety
///
/// This function never returns. All arguments must be valid user space addresses.
#[unsafe(naked)]
pub unsafe extern "C" fn x86_uspace_entry(
    arg1: usize,
    arg2: usize,
    sp: usize,
    pc: usize,
    rflags: u64,
) -> ! {
    naked_asm!(
        // Arguments on entry:
        // rdi = arg1
        // rsi = arg2
        // rdx = sp
        // rcx = pc
        // r8  = rflags

        // Push a fake 64-bit interrupt stack frame
        "push {user_ss}",      // ss
        "push rdx",            // sp (user stack)
        "push r8",             // rflags (user flags)
        "push {user_cs}",      // cs
        "push rcx",            // pc (user RIP)

        // Clear all general-purpose registers except rdi and rsi
        // which hold the user arguments
        "xor eax, eax",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "xor edx, edx",
        // Don't clear rdi or rsi - they have user arguments
        "xor ebp, ebp",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "xor r12d, r12d",
        "xor r13d, r13d",
        "xor r14d, r14d",
        "xor r15d, r15d",

        // We do not need to clear extended register state (SSE, AVX, etc.)
        // since the kernel only uses general-purpose registers, and the
        // extended state is initialized to a cleared state on thread creation.

        // Switch to user GS (swapgs reverses the effect from syscall entry)
        "swapgs",

        // Load user data segments
        // We use ax which was zeroed above
        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",

        // Return to user space using iretq
        "iretq",

        user_ss = const USER_DATA_SELECTOR,
        user_cs = const USER_CODE_64_SELECTOR,
    );
}

/// Enter user space with default RFLAGS
///
/// This is an alternative entry point that uses default RFLAGS value
/// (interrupts enabled) instead of a custom value.
///
/// # Arguments
///
/// * `arg1` - First argument (rdi)
/// * `arg2` - Second argument (rsi)
/// * `sp` - User stack pointer
/// * `pc` - User program counter
///
/// # Safety
///
/// This function never returns. All arguments must be valid user space addresses.
#[unsafe(naked)]
pub unsafe extern "C" fn x86_uspace_entry_simple(
    arg1: usize,
    arg2: usize,
    sp: usize,
    pc: usize,
) -> ! {
    naked_asm!(
        // Arguments on entry:
        // rdi = arg1
        // rsi = arg2
        // rdx = sp
        // rcx = pc

        // Set default rflags (interrupts enabled, IOPL 0)
        "push {user_ss}",
        "push rdx",             // sp
        "push {default_flags}", // rflags (IF = 1)
        "push {user_cs}",
        "push rcx",             // pc

        // Clear registers (except rdi, rsi)
        "xor eax, eax",
        "xor ebx, ebx",
        "xor ecx, ecx",
        "xor edx, edx",
        "xor ebp, ebp",
        "xor r8d, r8d",
        "xor r9d, r9d",
        "xor r10d, r10d",
        "xor r11d, r11d",
        "xor r12d, r12d",
        "xor r13d, r13d",
        "xor r14d, r14d",
        "xor r15d, r15d",

        "swapgs",

        "mov ds, ax",
        "mov es, ax",
        "mov fs, ax",
        "mov gs, ax",

        "iretq",

        user_ss = const USER_DATA_SELECTOR,
        user_cs = const USER_CODE_64_SELECTOR,
        default_flags = const DEFAULT_USER_RFLAGS,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selector_values() {
        assert_eq!(selectors::USER_CS, 0x2B);
        assert_eq!(selectors::USER_DS, 0x23);
        assert_eq!(selectors::USER_CS_32, 0x23);
        assert_eq!(selectors::NULL_SEL, 0);
    }

    #[test]
    fn test_default_rflags() {
        // Should have IF set (bit 9)
        assert_eq!(DEFAULT_USER_RFLAGS & (1 << 9), 1 << 9);
        // Should equal 0x202
        assert_eq!(DEFAULT_USER_RFLAGS, 0x202);
    }
}
