// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 Exception and Fault Handlers
//!
//! This module handles all x86-64 exceptions including page faults,
//! general protection faults, and debug exceptions.

use crate::arch::amd64::registers;
use crate::arch::amd64::registers::X86_FLAGS_AC;
use crate::arch::amd64::syscall::X86Iframe;

/// Page fault error code flags
pub mod pf_error {
    /// Page present
    pub const P: u64 = 1 << 0;
    /// Write access
    pub const W: u64 = 1 << 1;
    /// User mode
    pub const U: u64 = 1 << 2;
    /// Reserved bit set
    pub const RSV: u64 = 1 << 3;
    /// Instruction fetch
    pub const I: u64 = 1 << 4;
    /// Protection key violation
    pub const PK: u64 = 1 << 5;
    /// SGX violation
    pub const SGX: u64 = 1 << 15;
}

/// X86-64 exception vectors
pub mod exception_vector {
    pub const DEBUG: u64 = 1;
    pub const NMI: u64 = 2;
    pub const BREAKPOINT: u64 = 3;
    pub const OVERFLOW: u64 = 4;
    pub const BOUND_RANGE: u64 = 5;
    pub const INVALID_OP: u64 = 6;
    pub const DEVICE_NA: u64 = 7;
    pub const DOUBLE_FAULT: u64 = 8;
    pub const INVALID_TSS: u64 = 10;
    pub const SEGMENT_NP: u64 = 11;
    pub const STACK_FAULT: u64 = 12;
    pub const GP_FAULT: u64 = 13;
    pub const PAGE_FAULT: u64 = 14;
    pub const X87_FP_ERROR: u64 = 16;
    pub const ALIGNMENT_CHECK: u64 = 17;
    pub const MACHINE_CHECK: u64 = 18;
    pub const SIMD_FP_ERROR: u64 = 19;
    pub const VIRTUALIZATION: u64 = 20;
    pub const SECURITY: u64 = 30;
}

/// Page fault flags for VM
pub mod pf_flags {
    pub const WRITE: u32 = 1 << 0;
    pub const USER: u32 = 1 << 1;
    pub const INSTRUCTION: u32 = 1 << 2;
    pub const NOT_PRESENT: u32 = 1 << 3;
}

/// Check if the exception came from user mode
fn is_from_user(frame: &X86Iframe) -> bool {
    // Since we don't have user_cs in X86Iframe, we need another way to check
    // For now, we'll use a placeholder that always returns false
    // TODO: Implement proper user mode detection
    false
}

/// Check if an address is likely a user-space address
///
/// This is a simple heuristic - user addresses are typically in the
/// canonical lower half (0x0000_0000_0000_0000 - 0x0000_7FFF_FFFF_FFFF)
pub fn is_user_address(addr: usize) -> bool {
    // User addresses are in the lower half
    addr < 0x0000_8000_0000_0000
}

/// Dump the fault frame for debugging
fn dump_fault_frame(frame: &X86Iframe) {
    let cr2 = unsafe { registers::x86_get_cr2() };

    // Note: We can't use println in the kernel without proper setup
    // In a real implementation, this would use the kernel's debug output

    // For now, this is just a placeholder
    let _ = (frame, cr2);

    // TODO: Implement proper debug output
    // println!("CS:  {:#18x} RIP: {:#18x} EFL: {:#18x} CR2: {:#18x}", ...);
}

/// Dump page fault error information
pub fn x86_dump_pfe(frame: &X86Iframe, cr2: u64, err_code: u64) {
    // TODO: Implement proper debug output
    let _ = (frame, cr2, err_code);

    // Extract error code information
    let access_type = if err_code & pf_error::W != 0 { "write" } else { "read" };
    let mode = if err_code & pf_error::U != 0 { "user" } else { "supervisor" };
    let fetch_type = if err_code & pf_error::I != 0 { "instruction" } else { "data" };
    let rsv = if err_code & pf_error::RSV != 0 { " rsv" } else { "" };
    let present = if err_code & pf_error::P != 0 {
        "protection violation"
    } else {
        "page not present"
    };

    // In a real implementation, this would log to debug output
    let _ = (access_type, mode, fetch_type, rsv, present);
}

/// Fatal page fault handler - halts the system
pub fn x86_fatal_pfe_handler(frame: &X86Iframe, cr2: u64, err_code: u64) -> ! {
    x86_dump_pfe(frame, cr2, err_code);

    // TODO: Implement proper panic handling
    exception_die(frame, "fatal page fault, halting\n");
}

/// Page fault handler
///
/// # Arguments
///
/// * `frame` - The interrupt frame
/// * `error_code` - Page fault error code
///
/// # Returns
///
/// Ok(()) if the fault was handled, Err otherwise
pub fn x86_pfe_handler(frame: &mut X86Iframe, error_code: u64) -> Result<(), ()> {
    let va = unsafe { registers::x86_get_cr2() } as usize;

    // Check for flags we're not prepared to handle
    let unhandled_bits = error_code & !(pf_error::I | pf_error::U | pf_error::W | pf_error::P);
    if unhandled_bits != 0 {
        // TODO: Log unhandled error code bits
        return Err(());
    }

    // Check for potential SMAP failure
    let supervisor_access = error_code & pf_error::U == 0;
    let page_present = error_code & pf_error::P != 0;
    let ac_clear = (frame.flags & X86_FLAGS_AC) == 0;
    let user_addr = is_user_address(va);

    // TODO: Check if SMAP is enabled
    // let smap_enabled = unsafe { feature::x86_feature_smap() };

    if supervisor_access && page_present && ac_clear && user_addr {
        // TODO: Log potential SMAP failure
        return Err(());
    }

    // Convert PF error codes to page fault flags
    let mut flags = 0u32;
    if error_code & pf_error::W != 0 {
        flags |= pf_flags::WRITE;
    }
    if error_code & pf_error::U != 0 {
        flags |= pf_flags::USER;
    }
    if error_code & pf_error::I != 0 {
        flags |= pf_flags::INSTRUCTION;
    }
    if error_code & pf_error::P == 0 {
        flags |= pf_flags::NOT_PRESENT;
    }

    // Call the high level page fault handler
    // TODO: Implement vmm_page_fault_handler
    // let pf_err = vmm_page_fault_handler(va, flags);
    // if pf_err == ZX_OK {
    //     return Ok(());
    // }

    // Let high level code deal with user space faults
    if is_from_user(frame) {
        // TODO: Dispatch user exception
        // For now, return error to trigger signal to user process
        return Err(());
    }

    // Fall through to fatal path
    Err(())
}

/// Debug exception handler
pub fn x86_debug_handler(frame: &mut X86Iframe) {
    // TODO: Implement debug exception handling
    let _ = frame;
    exception_die(frame, "unhandled hw breakpoint, halting\n");
}

/// Breakpoint exception handler (INT 3)
pub fn x86_breakpoint_handler(frame: &mut X86Iframe) {
    // TODO: Implement breakpoint exception handling
    let _ = frame;
    exception_die(frame, "unhandled sw breakpoint, halting\n");
}

/// General protection fault handler
pub fn x86_gpf_handler(frame: &mut X86Iframe) {
    // TODO: Implement GPF handling
    let _ = frame;
    exception_die(frame, "unhandled gpf, halting\n");
}

/// Invalid opcode handler
pub fn x86_invop_handler(frame: &mut X86Iframe) {
    // TODO: Implement invalid opcode handling
    let _ = frame;
    exception_die(frame, "invalid opcode, halting\n");
}

/// Double fault handler
pub fn x86_df_handler(frame: &X86Iframe) {
    // Do not give the user exception handler the opportunity to handle double faults
    let _ = frame;
    exception_die(frame, "double fault, halting\n");
}

/// NMI handler
pub fn x86_nmi_handler(_frame: &X86Iframe) {
    // NMI handler - typically used for watchdog or hardware diagnostics
    // TODO: Implement proper NMI handling
}

/// Unhandled exception handler
pub fn x86_unhandled_exception(frame: &mut X86Iframe) {
    // TODO: Implement unhandled exception handling
    let _ = frame;
    exception_die(frame, "unhandled exception, halting\n");
}

/// Fatal exception handler - prints diagnostic and halts
fn exception_die(frame: &X86Iframe, msg: &str) -> ! {
    // TODO: Implement proper panic handling:
    // - platform_panic_start() to notify other subsystems
    // - Dump user stack if from user space
    // - Save crash log to persistent storage
    // - Call platform-specific halt

    // For user exceptions, try to dump user stack
    if is_from_user(frame) {
        // TODO: Implement user-space stack unwinding
    }

    // Print exception information (if debug output is available)
    let _ = (frame, msg);

    // Halt the system
    loop {
        unsafe { registers::x86_hlt() };
    }
}

/// Main exception dispatch handler
///
/// Called from assembly exception entry point
///
/// # Arguments
///
/// * `frame` - The interrupt frame
/// * `vector` - Exception vector number
#[no_mangle]
pub unsafe extern "C" fn x86_exception_handler(frame: *mut X86Iframe, vector: u64) {
    let frame = &mut *frame;

    match vector {
        exception_vector::DEBUG => {
            x86_debug_handler(frame);
        }
        exception_vector::NMI => {
            x86_nmi_handler(frame);
        }
        exception_vector::BREAKPOINT => {
            x86_breakpoint_handler(frame);
        }
        exception_vector::INVALID_OP => {
            x86_invop_handler(frame);
        }
        exception_vector::DEVICE_NA => {
            exception_die(frame, "device na fault\n");
        }
        exception_vector::DOUBLE_FAULT => {
            x86_df_handler(frame);
        }
        exception_vector::GP_FAULT => {
            x86_gpf_handler(frame);
        }
        exception_vector::PAGE_FAULT => {
            // For page faults, we need to handle the error code
            // The error code would be passed separately from the frame
            // For now, use a placeholder error code
            let error_code = pf_error::P; // Present flag

            if x86_pfe_handler(frame, error_code).is_err() {
                let cr2 = registers::x86_get_cr2();
                x86_fatal_pfe_handler(frame, cr2 as u64, error_code);
            }
        }
        _ => {
            x86_unhandled_exception(frame);
        }
    }
}

/// Architecture exception context for user-space dispatch
#[repr(C)]
pub struct ArchExceptionContext {
    pub is_page_fault: bool,
    pub frame: *const X86Iframe,
    pub cr2: u64,
}
