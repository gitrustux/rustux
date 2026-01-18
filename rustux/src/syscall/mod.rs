// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! System Call Interface
//!
//! This module provides the unified system call ABI for the Rustux kernel.
//! The syscall ABI is stable across all architectures (ARM64, AMD64, RISC-V).
//!
//! # Design Rules
//!
//! - **Stability**: Syscall numbers & semantics frozen across architectures
//! - **Object-based**: All operations on handles with rights
//! - **Deterministic**: Same inputs → same outputs → same errors
//! - **No arch leakage**: CPU differences hidden below ABI
//!
//! # Calling Convention
//!
//! | Architecture | Syscall Instruction | Arg Registers | Return |
//! |--------------|---------------------|---------------|--------|
//! | ARM64 | `svc #0` | x0-x6 | x0 |
//! | AMD64 | `syscall` | rdi, rsi, rdx, r10, r8, r9 | rax |
//! | RISC-V | `ecall` | a0-a6 | a0 |
//!
//! # Error Return Convention
//!
//! ```text
//! Success: return value in r0/rax/a0 (positive or zero)
//! Failure: return negative error code
//! ```

use crate::arch::amd64::mm::RxStatus;

// ============================================================================
// Common Syscall Types
// ============================================================================

/// Interrupt frame for syscall/exception handling
///
/// This structure represents the CPU state at the time of a syscall
/// or exception. It's used by the syscall entry code to preserve
/// and restore user state.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86Iframe {
    /// General purpose registers
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub rax: u64,  // syscall number / return value
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    /// User stack pointer
    pub user_sp: u64,

    /// Instruction pointer
    pub ip: u64,

    /// Flags register
    pub flags: u64,
}

impl X86Iframe {
    /// Create a new zeroed interrupt frame
    pub const fn new() -> Self {
        Self {
            rdi: 0,
            rsi: 0,
            rdx: 0,
            r10: 0,
            r8: 0,
            r9: 0,
            rax: 0,
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            user_sp: 0,
            ip: 0,
            flags: 0,
        }
    }
}

/// Syscall general registers
///
/// This contains the registers used for syscall arguments.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct X86SyscallGeneralRegs {
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub rax: u64,  // syscall number / return value
    pub r11: u64,  // saved user RFLAGS
    pub rcx: u64,  // user RIP
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rsp: u64,  // user RSP
    pub rip: u64,  // user RIP
    pub rflags: u64,  // user RFLAGS
}

impl X86SyscallGeneralRegs {
    /// Create a new zeroed syscall register struct
    pub const fn new() -> Self {
        Self {
            rdi: 0,
            rsi: 0,
            rdx: 0,
            r10: 0,
            r8: 0,
            r9: 0,
            rax: 0,
            r11: 0,
            rcx: 0,
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp: 0,
            rip: 0,
            rflags: 0,
        }
    }
}

/// Syscall statistics (for debugging/monitoring)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SyscallStats {
    /// Number of times this syscall was called
    pub count: u64,
    /// Total time spent in this syscall (TSC ticks)
    pub total_time: u64,
    /// Maximum time spent in a single call (TSC ticks)
    pub max_time: u64,
}

impl SyscallStats {
    /// Create a new zeroed syscall stats struct
    pub const fn new() -> Self {
        Self {
            count: 0,
            total_time: 0,
            max_time: 0,
        }
    }
}

/// Per-syscall statistics
static mut SYSCALL_STATS: [SyscallStats; 1000] = [SyscallStats::new(); 1000];

/// Record a syscall invocation
fn record_syscall(num: u32) {
    unsafe {
        SYSCALL_STATS[num as usize].count += 1;
    }
}

/// Get syscall statistics for a syscall
pub unsafe fn get_syscall_stats(syscall_num: u32) -> Option<&'static SyscallStats> {
    if (syscall_num as usize) < SYSCALL_STATS.len() {
        Some(&SYSCALL_STATS[syscall_num as usize])
    } else {
        None
    }
}

// Syscall numbers (Stable v1)
//
// These numbers are frozen as part of the stable ABI v1.
// DO NOT change existing numbers - only append new syscalls.

// Syscall return type
pub type SyscallRet = isize;

/// System call arguments
///
/// This structure holds the arguments passed to a system call.
/// The layout is designed to match the calling conventions:
/// - ARM64: x0-x5 → args[0-5], syscall number in x8
/// - AMD64: rdi,rsi,rdx,r10,r8,r9 → args[0-5], syscall number in rax
/// - RISC-V: a0-a5 → args[0-5], syscall number in a7
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SyscallArgs {
    /// Syscall number
    pub number: u32,

    /// Arguments (up to 6)
    pub args: [usize; 6],
}

impl SyscallArgs {
    /// Create new syscall arguments
    pub const fn new(number: u32, args: [usize; 6]) -> Self {
        Self { number, args }
    }

    /// Get argument at index
    pub const fn arg(&self, index: usize) -> usize {
        if index < 6 {
            self.args[index]
        } else {
            0
        }
    }

    /// Get argument as u32
    pub const fn arg_u32(&self, index: usize) -> u32 {
        self.arg(index) as u32
    }

    /// Get argument as u64
    pub const fn arg_u64(&self, index: usize) -> u64 {
        self.arg(index) as u64
    }

    /// Get argument as i64
    pub const fn arg_i64(&self, index: usize) -> i64 {
        self.arg(index) as i64
    }
}

/// Convert error code to negative return value
#[inline]
pub const fn err_to_ret(err: RxStatus) -> SyscallRet {
    -(err as SyscallRet)
}

/// Convert success value to return value
#[inline]
pub const fn ok_to_ret(val: usize) -> SyscallRet {
    val as SyscallRet
}

/// Convert success value (isize) to return value
#[inline]
pub const fn ok_to_ret_isize(val: isize) -> SyscallRet {
    val
}

/// ============================================================================
/// Syscall Dispatcher
/// ============================================================================

/// System call dispatcher
///
/// This function is called from the architecture-specific syscall entry point.
/// It validates the syscall number and dispatches to the appropriate handler.
///
/// # Arguments
///
/// * `args` - System call arguments
///
/// # Returns
///
/// System call return value (positive/zero for success, negative for error)
///
/// # Calling Convention
///
/// This function uses the C ABI and is callable from assembly.
#[no_mangle]
pub extern "C" fn syscall_dispatch(args: SyscallArgs) -> SyscallRet {
    let num = args.number;

    // Dispatch to handler based on syscall number
    // For now, most syscalls return NOT_IMPLEMENTED
    // We'll implement them incrementally as needed

    match num {
        // Process & Thread (0x01-0x0F)
        0x01 => sys_process_create(args),
        0x02 => sys_process_start(args),
        0x03 => sys_thread_create(args),
        0x04 => sys_thread_start(args),
        0x05 => sys_thread_exit(args),
        0x06 => sys_process_exit(args),
        0x07 => sys_handle_close(args),

        // Memory / VMO (0x10-0x1F)
        0x10 => sys_vmo_create(args),
        0x11 => sys_vmo_read(args),
        0x12 => sys_vmo_write(args),
        0x13 => sys_vmo_clone(args),
        0x14 => sys_vmar_map(args),
        0x15 => sys_vmar_unmap(args),
        0x16 => sys_vmar_protect(args),

        // IPC & Sync (0x20-0x2F)
        0x20 => sys_channel_create(args),
        0x21 => sys_channel_write(args),
        0x22 => sys_channel_read(args),
        0x23 => sys_event_create(args),
        0x24 => sys_eventpair_create(args),
        0x25 => sys_object_signal(args),
        0x26 => sys_object_wait_one(args),
        0x27 => sys_object_wait_many(args),

        // Jobs & Handles (0x30-0x3F)
        0x30 => sys_job_create(args),
        0x31 => sys_handle_duplicate(args),
        0x32 => sys_handle_transfer(args),

        // Time (0x40-0x4F)
        0x40 => sys_clock_get(args),
        0x41 => sys_timer_create(args),
        0x42 => sys_timer_set(args),
        0x43 => sys_timer_cancel(args),

        _ => {
            // Unknown syscall
            err_to_ret(RxStatus::ERR_NOT_SUPPORTED)
        }
    }
}

/// ============================================================================
/// Syscall Handler Implementations (Stubs)
/// ============================================================================

/// Stub for syscall handlers not yet implemented
macro_rules! syscall_stub {
    ($name:ident) => {
        fn $name(args: SyscallArgs) -> SyscallRet {
            // TODO: Implement $name
            let _ = args;
            err_to_ret(RxStatus::ERR_NOT_SUPPORTED)
        }
    };
}

// Process & Thread syscalls
syscall_stub!(sys_process_create);
syscall_stub!(sys_process_start);
syscall_stub!(sys_thread_create);
syscall_stub!(sys_thread_start);
syscall_stub!(sys_thread_exit);
syscall_stub!(sys_process_exit);

fn sys_handle_close(args: SyscallArgs) -> SyscallRet {
    let handle = args.arg_u32(0);
    // TODO: Implement handle close
    let _ = handle;
    ok_to_ret(0)
}

// Memory / VMO syscalls
syscall_stub!(sys_vmo_create);
syscall_stub!(sys_vmo_read);
syscall_stub!(sys_vmo_write);
syscall_stub!(sys_vmo_clone);
syscall_stub!(sys_vmar_map);
syscall_stub!(sys_vmar_unmap);
syscall_stub!(sys_vmar_protect);

// IPC & Sync syscalls
syscall_stub!(sys_channel_create);
syscall_stub!(sys_channel_write);
syscall_stub!(sys_channel_read);
syscall_stub!(sys_event_create);
syscall_stub!(sys_eventpair_create);
syscall_stub!(sys_object_signal);
syscall_stub!(sys_object_wait_one);
syscall_stub!(sys_object_wait_many);

// Jobs & Handles syscalls
syscall_stub!(sys_job_create);
syscall_stub!(sys_handle_duplicate);
syscall_stub!(sys_handle_transfer);

// Time syscalls
fn sys_clock_get(args: SyscallArgs) -> SyscallRet {
    let _clock_id = args.arg_u32(0);
    // Return current time in nanoseconds (placeholder)
    // Use the TSC for now
    use crate::arch::amd64::tsc;
    let time_ns = tsc::tsc_to_ns(unsafe { tsc::rdtsc() });
    ok_to_ret_isize(time_ns as isize)
}

syscall_stub!(sys_timer_create);
syscall_stub!(sys_timer_set);
syscall_stub!(sys_timer_cancel);

/// ============================================================================
/// Module Initialization
/// ============================================================================

/// Initialize the syscall subsystem
pub fn init() {
    // Syscall subsystem initialization
    // TODO: Set up syscall tables, etc.
}

/// ============================================================================
/// Syscall Numbers
/// ============================================================================

/// System call numbers (Stable v1)
pub mod number {
    /// Process & Thread (0x01-0x0F)
    pub const PROCESS_CREATE: u32 = 0x01;
    pub const PROCESS_START: u32 = 0x02;
    pub const THREAD_CREATE: u32 = 0x03;
    pub const THREAD_START: u32 = 0x04;
    pub const THREAD_EXIT: u32 = 0x05;
    pub const PROCESS_EXIT: u32 = 0x06;
    pub const HANDLE_CLOSE: u32 = 0x07;

    /// Memory / VMO (0x10-0x1F)
    pub const VMO_CREATE: u32 = 0x10;
    pub const VMO_READ: u32 = 0x11;
    pub const VMO_WRITE: u32 = 0x12;
    pub const VMO_CLONE: u32 = 0x13;
    pub const VMAR_MAP: u32 = 0x14;
    pub const VMAR_UNMAP: u32 = 0x15;
    pub const VMAR_PROTECT: u32 = 0x16;

    /// IPC & Sync (0x20-0x2F)
    pub const CHANNEL_CREATE: u32 = 0x20;
    pub const CHANNEL_WRITE: u32 = 0x21;
    pub const CHANNEL_READ: u32 = 0x22;
    pub const EVENT_CREATE: u32 = 0x23;
    pub const EVENTPAIR_CREATE: u32 = 0x24;
    pub const OBJECT_SIGNAL: u32 = 0x25;
    pub const OBJECT_WAIT_ONE: u32 = 0x26;
    pub const OBJECT_WAIT_MANY: u32 = 0x27;

    /// Jobs & Handles (0x30-0x3F)
    pub const JOB_CREATE: u32 = 0x30;
    pub const HANDLE_DUPLICATE: u32 = 0x31;
    pub const HANDLE_TRANSFER: u32 = 0x32;

    /// Time (0x40-0x4F)
    pub const CLOCK_GET: u32 = 0x40;
    pub const TIMER_CREATE: u32 = 0x41;
    pub const TIMER_SET: u32 = 0x42;
    pub const TIMER_CANCEL: u32 = 0x43;

    /// Maximum defined syscall number
    pub const MAX_SYSCALL: u32 = 0x43;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_syscall_args() {
        let args = SyscallArgs::new(0x10, [1, 2, 3, 4, 5, 6]);
        assert_eq!(args.number, 0x10);
        assert_eq!(args.arg(0), 1);
        assert_eq!(args.arg(5), 6);
        assert_eq!(args.arg(10), 0); // Out of range
    }

    #[test]
    fn test_ret_conversions() {
        assert_eq!(ok_to_ret(42), 42);
        assert_eq!(err_to_ret(RxStatus::ERR_NO_MEMORY), -(RxStatus::ERR_NO_MEMORY as SyscallRet));
        assert_eq!(ok_to_ret_isize(-1), -1);
        assert_eq!(ok_to_ret_isize(100), 100);
    }

    #[test]
    fn test_syscall_numbers() {
        assert_eq!(number::PROCESS_CREATE, 0x01);
        assert_eq!(number::VMO_CREATE, 0x10);
        assert_eq!(number::CHANNEL_CREATE, 0x20);
        assert_eq!(number::JOB_CREATE, 0x30);
        assert_eq!(number::CLOCK_GET, 0x40);
    }
}
