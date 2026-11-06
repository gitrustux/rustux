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

pub mod driver;
pub mod memory;
pub mod object;
pub mod system;
pub mod task;
pub mod user;

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

/// Per-syscall call counts
static SYSCALL_COUNTS: [core::sync::atomic::AtomicU64; 256] = [const { core::sync::atomic::AtomicU64::new(0) }; 256];

/// Record a syscall invocation
#[allow(dead_code)]
fn record_syscall(num: u32) {
    if let Some(c) = SYSCALL_COUNTS.get(num as usize) {
        c.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Statistics for a syscall (only the count is recorded so far)
pub fn get_syscall_stats(syscall_num: u32) -> Option<SyscallStats> {
    SYSCALL_COUNTS.get(syscall_num as usize).map(|c| SyscallStats {
        count: c.load(core::sync::atomic::Ordering::Relaxed),
        ..SyscallStats::new()
    })
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
        0x01 => task::sys_process_create(args),
        0x02 => task::sys_process_start(args),
        0x04 => task::sys_thread_start(args),
        0x05 => task::sys_thread_exit(args),
        0x06 => task::sys_process_exit(args),
        0x07 => sys_handle_close(args),
        0x08 => task::sys_thread_create(args),
        0x09 => task::sys_task_kill(args),
        0x0A => task::sys_object_get_info(args),
        0x0C => task::sys_object_get_child(args),
        0x0D => task::sys_object_get_property(args),
        0x0E => task::sys_object_set_property(args),

        // Memory / VMO (0x10-0x1F)
        0x10 => memory::sys_vmo_create(args),
        0x11 => memory::sys_vmo_read(args),
        0x12 => memory::sys_vmo_write(args),
        0x13 => memory::sys_vmo_clone(args),
        0x14 => memory::sys_vmar_map(args),
        0x15 => memory::sys_vmar_unmap(args),
        0x16 => memory::sys_vmar_protect(args),
        0x17 => memory::sys_vmo_get_size(args),
        0x18 => memory::sys_vmo_replace_as_executable(args),
        0x19 => memory::sys_vmo_create_physical(args),
        0x1A => memory::sys_vmo_set_cache_policy(args),
        0x1B => memory::sys_vmo_create_contiguous(args),

        // IPC & Sync (0x20-0x2F)
        0x20 => object::sys_channel_create(args),
        0x21 => object::sys_channel_write(args),
        0x22 => object::sys_channel_read(args),
        0x23 => object::sys_event_create(args),
        0x24 => object::sys_eventpair_create(args),
        0x25 => object::sys_object_signal(args),
        0x26 => object::sys_object_wait_one(args),
        0x27 => object::sys_object_wait_many(args),
        0x28 => object::sys_object_signal_peer(args),
        0x29 => object::sys_object_get_type(args),
        0x2A => object::sys_socket_create(args),
        0x2B => object::sys_socket_write(args),
        0x2C => object::sys_socket_read(args),

        // Jobs & Handles (0x30-0x3F)
        0x30 => task::sys_job_create(args),
        0x31 => object::sys_handle_duplicate(args),
        0x32 => task::sys_handle_replace(args),

        // Time (0x40-0x4F)
        0x40 => system::sys_clock_get(args),
        0x41 => object::sys_timer_create(args),
        0x42 => object::sys_timer_set(args),
        0x43 => object::sys_timer_cancel(args),
        0x44 => system::sys_clock_set_utc(args),

        // Debug (0x50-0x5F)
        0x50 => sys_debug_write(args),
        // debug_cpu(): index of the CPU running the caller (tests)
        0x51 => ok_to_ret(crate::arch::amd64::percpu::this().index),
        // debug_cpu_count(): CPUs running the scheduler
        0x52 => ok_to_ret(crate::arch::amd64::percpu::online_count()),
        0x53 => driver::sys_debuglog_create(args),
        0x54 => driver::sys_debuglog_write(args),
        0x55 => driver::sys_debuglog_read(args),

        // Drivers (0x80-0x8F)
        0x80 => driver::sys_interrupt_create(args),
        0x81 => driver::sys_interrupt_wait(args),
        0x82 => driver::sys_interrupt_ack(args),
        0x83 => driver::sys_interrupt_destroy(args),
        0x84 => driver::sys_ioports_request(args),
        0x85 => driver::sys_ioports_release(args),
        0x86 => driver::sys_resource_create(args),
        0x87 => driver::sys_bti_create(args),
        0x88 => driver::sys_bti_pin(args),
        0x89 => driver::sys_pmt_unpin(args),

        // System (0x90-0x9F)
        0x90 => system::sys_system_get_version(args),
        0x91 => system::sys_system_get_stats(args),
        0x92 => system::sys_system_powerctl(args),
        0x93 => driver::sys_acpi_table_read(args),
        0x94 => driver::sys_usb_list(args),
        0x95 => driver::sys_usb_open(args),
        0x96 => driver::sys_usb_bulk(args),
        0x97 => driver::sys_usb_control(args),
        0x98 => driver::sys_usb_clear_halt(args),
        0x99 => driver::sys_usb_reset(args),

        // yield(): give up the CPU (tests, and waiting for a full console)
        0x72 => sys_yield(args),

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

fn sys_handle_close(args: SyscallArgs) -> SyscallRet {
    object::sys_handle_close(args)
}

// Memory / VMO syscalls

// IPC & Sync syscalls

// Jobs & Handles syscalls



// Debug syscalls
/// Debug write syscall - writes a string to the debug console
///
/// Arguments:
///   arg0: pointer to string (userspace virtual address)
///   arg1: length of string
///
/// Returns: number of bytes written, or negative error code
fn sys_debug_write(args: SyscallArgs) -> SyscallRet {
    let (ptr, len) = (args.arg(0), args.arg(1));
    match user::read_bytes(ptr, len.min(4096)) {
        Ok(bytes) => {
            for &c in &bytes {
                unsafe { crate::klog::put(c) };
            }
            ok_to_ret_isize(bytes.len() as isize)
        }
        Err(e) => err_to_ret(e),
    }
}

// ============================================================================
// Yield
// ============================================================================

/// Yield CPU to scheduler
///
/// Arguments: none
///
/// Returns: 0 on success, negative error code on failure
///
/// This syscall voluntarily gives up the CPU to other processes.
/// It calls the scheduler to find and switch to the next runnable process.
fn sys_yield(_args: SyscallArgs) -> SyscallRet {
    use crate::sched::round_robin;

    match round_robin::yield_cpu() {
        Ok(()) => ok_to_ret(0),
        Err(e) => {
            // Debug output
            let msg = b"[YIELD] Failed: ";
            for &b in msg {
                unsafe {
                    crate::klog::put(b);
                }
            }
            for b in e.as_bytes() {
                unsafe {
                    crate::klog::put(*b);
                }
            }
            let msg = b"\n";
            for &b in msg {
                unsafe {
                    crate::klog::put(b);
                }
            }
            err_to_ret(RxStatus::ERR_INVALID_ARGS)
        }
    }
}

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

    /// Debug (0x50-0x5F)
    pub const DEBUG_WRITE: u32 = 0x50;

    /// Sockets
    pub const SOCKET_CREATE: u32 = 0x2A;
    pub const SOCKET_WRITE: u32 = 0x2B;
    pub const SOCKET_READ: u32 = 0x2C;

    pub const YIELD: u32 = 0x72;

    /// Maximum defined syscall number
    pub const MAX_SYSCALL: u32 = 0x72;
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
