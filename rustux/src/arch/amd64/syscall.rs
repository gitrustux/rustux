// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 System Call Architecture Support
//!
//! This module provides x86-64-specific support for system calls,
//! including MSR initialization and register handling.
//!
//! The x86-64 syscall ABI uses:
//! - rax: syscall number
//! - rdi, rsi, rdx, r10, r8, r9: arguments (in order)
//! - rcx: return address to user space
//! - r11: saved RFLAGS
//!
//! The syscall is initiated via the `syscall` instruction which:
//! 1. Saves rcx to user RIP (return address)
//! 2. Saves r11 to user RFLAGS
//! 3. Loads kernel CS/RIP from IA32_LSTAR MSR
//! 4. Loads kernel SS from IA32_STAR MSR

use crate::arch::amd64::registers::{self, msr, rflags};
use crate::syscall::{self as sys, SyscallArgs, SyscallRet};

/// Re-export syscall types from the main syscall module
pub use crate::syscall::{X86Iframe, X86SyscallGeneralRegs, SyscallStats};

/// ============================================================================
/// MSR Setup for Syscalls
/// ============================================================================

/// Initialize MSRs for syscall support
///
/// This should be called during kernel initialization to set up
/// the MSRs that control syscall behavior.
///
/// # Safety
///
/// This function modifies MSRs and should only be called once
/// during initialization.
pub unsafe fn x86_syscall_init() {
    // IA32_STAR - System Call Target Address
    // Bits [63:48] = Kernel CS (must be 0x08 for 64-bit kernel)
    // Bits [47:32] = User CS (must be 0x1B for user mode)
    // This MSR is used for compatibility mode (32-bit) syscalls
    let star_value: u64 = (0x08u64 << 32) | (0x1Bu64 << 48);
    registers::write_msr(msr::IA32_STAR, star_value);

    // IA32_LSTAR - IA32-e Mode System Call Target Address
    // This is the RIP where syscalls enter in 64-bit mode
    // Set to the architecture-specific syscall entry point
    extern "C" {
        fn x86_64_syscall_entry();
    }
    registers::write_msr(msr::IA32_LSTAR, x86_64_syscall_entry as u64);

    // IA32_FMASK - System Call Flag Mask
    // Masks RFLAGS bits that are cleared on syscall entry
    // We mask IF (interrupt enable) to disable interrupts during syscall
    // and other flags to maintain consistent kernel state
    const FMASK_VALUE: u64 = rflags::IF | rflags::TF | rflags::DF;
    registers::write_msr(msr::IA32_FMASK, FMASK_VALUE);

    // IA32_EFER - Enable SCE (System Call Extensions)
    let mut efer = registers::read_msr(msr::IA32_EFER);
    efer |= registers::efer::SCE;
    registers::write_msr(msr::IA32_EFER, efer);
}

/// ============================================================================
/// Architecture-Specific Syscall Entry Point
/// ============================================================================

/// AMD64 syscall entry point
///
/// This function is called from the syscall instruction in user space.
/// It properly saves/restores registers and calls the syscall dispatcher.
///
/// # Safety
///
/// This function must preserve all registers except for the syscall
/// arguments and return value.
#[no_mangle]
pub unsafe extern "C" fn x86_64_syscall_entry(
    rdi: usize,
    rsi: usize,
    rdx: usize,
    r10: usize,
    r8: usize,
    r9: usize,
    rax: u32,
) -> SyscallRet {
    // Debug: Indicate syscall entry
    {
        let msg = b"[SYSCALL] Entry, num=0x";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
        let mut n = rax;
        let mut buf = [0u8; 8];
        let mut i = 0;
        if n == 0 {
            buf[i] = b'0';
            i += 1;
        } else {
            while n > 0 {
                let digit = (n & 0xF) as u8;
                buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                n >>= 4;
                i += 1;
            }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b"\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    // Create syscall arguments structure
    let args = SyscallArgs::new(rax, [rdi, rsi, rdx, r10, r8, r9]);

    // Call the main syscall dispatcher
    let result = sys::syscall_dispatch(args);

    // Debug: Indicate syscall exit
    {
        let msg = b"[SYSCALL] Return\n";
        for &b in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
        }
    }

    result
}
