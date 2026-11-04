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

pub mod fd;
pub mod driver;
pub mod memory;
pub mod object;
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
        0x03 => task::sys_spawn(args),
        0x04 => task::sys_thread_start(args),
        0x05 => task::sys_thread_exit(args),
        0x06 => task::sys_process_exit(args),
        0x07 => sys_handle_close(args),
        0x08 => task::sys_thread_create(args),
        0x09 => task::sys_task_kill(args),
        0x0A => task::sys_object_get_info(args),
        0x0B => task::sys_spawn_process(args),

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

        // Jobs & Handles (0x30-0x3F)
        0x30 => task::sys_job_create(args),
        0x31 => object::sys_handle_duplicate(args),
        0x32 => task::sys_handle_replace(args),

        // Time (0x40-0x4F)
        0x40 => sys_clock_get(args),
        0x41 => object::sys_timer_create(args),
        0x42 => object::sys_timer_set(args),
        0x43 => object::sys_timer_cancel(args),

        // Debug (0x50-0x5F)
        0x50 => sys_debug_write(args),
        // debug_cpu(): index of the CPU running the caller (tests)
        0x51 => ok_to_ret(crate::arch::amd64::percpu::this().index),
        // debug_cpu_count(): CPUs running the scheduler
        0x52 => ok_to_ret(crate::arch::amd64::percpu::online_count()),
        0x53 => driver::sys_debuglog_create(args),
        0x54 => driver::sys_debuglog_write(args),

        // I/O (0x60-0x6F) - Phase 5A
        0x60 => sys_write(args),
        0x61 => sys_read(args),
        0x62 => sys_open(args),
        0x63 => sys_close(args),
        0x64 => sys_lseek(args),

        // Process Info (0x70-0x7F) - Phase 5A
        // Drivers (0x80-0x8F)
        0x80 => driver::sys_interrupt_create(args),
        0x81 => driver::sys_interrupt_wait(args),
        0x82 => driver::sys_interrupt_ack(args),
        0x83 => driver::sys_interrupt_destroy(args),
        0x84 => driver::sys_ioports_request(args),
        0x85 => driver::sys_ioports_release(args),

        0x70 => sys_getpid(args),
        0x71 => sys_getppid(args),
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

// Time syscalls
fn sys_clock_get(args: SyscallArgs) -> SyscallRet {
    let _clock_id = args.arg_u32(0);
    // Return current time in nanoseconds (placeholder)
    // Use the TSC for now
    use crate::arch::amd64::tsc;
    let time_ns = tsc::tsc_to_ns(unsafe { tsc::rdtsc() });
    ok_to_ret_isize(time_ns as isize)
}


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
                unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)) };
            }
            ok_to_ret_isize(bytes.len() as isize)
        }
        Err(e) => err_to_ret(e),
    }
}

// ============================================================================
// I/O Syscalls (Phase 5A)
// ============================================================================

/// Write to file descriptor
///
/// Arguments:
///   arg0: file descriptor (fd)
///   arg1: pointer to buffer
///   arg2: length to write
///
/// Returns: number of bytes written, or negative error code
///
/// File descriptor mapping:
///   fd 0: stdin (write not allowed)
///   fd 1: stdout (kernel debug console, port 0xE9)
///   fd 2: stderr (same as stdout)
///   fd 3+: reserved for files (Phase 5C)
fn sys_write(args: SyscallArgs) -> SyscallRet {
    let fd = args.arg(0);
    let (ptr, len) = (args.arg(1), args.arg(2));

    // stdout/stderr: the kernel console. Programs built on the C library
    // print through their stdout handle instead; this call goes away in
    // step 8.
    if fd == 1 || fd == 2 {
        let bytes = match user::read_bytes(ptr, len.min(65536)) {
            Ok(b) => b,
            Err(e) => return err_to_ret(e),
        };
        if crate::drivers::display::is_initialized() {
            crate::drivers::display::write_bytes(&bytes);
        } else {
            for &c in &bytes {
                unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") c, options(nomem, nostack)) };
            }
        }
        return ok_to_ret_isize(bytes.len() as isize);
    }

    // stdin - cannot write
    if fd == 0 {
        return err_to_ret(RxStatus::ERR_INVALID_ARGS);
    }

    // For other file descriptors (fd 3+), return not implemented for now
    // Future: Write to ramdisk files
    ok_to_ret_isize(len as isize)
}

/// Read from file descriptor
///
/// Arguments:
///   arg0: file descriptor (fd)
///   arg1: pointer to buffer
///   arg2: length to read
///
/// Returns: number of bytes read, or negative error code
///
/// Read from a file descriptor
///
/// For stdin (fd 0): Blocks waiting for keyboard input, returns one character at a time
/// For files: Reads from ramdisk files
/// For stdout/stderr: Returns error (not readable)
fn sys_read(args: SyscallArgs) -> SyscallRet {
    use crate::syscall::fd::{FdKind, FileDescriptor};
    use crate::process::table::PROCESS_TABLE;

    let fd = args.arg(0) as u8;
    let ptr = args.arg_u64(1) as *mut u8;
    let len = args.arg(2);

    // Get the current process
    let file_info = {
        let mut table = PROCESS_TABLE.lock();
        let current = match table.current_mut() {
            Some(p) => p,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
        };

        // Get the file descriptor
        let file_desc = match current.fd_table.get(fd) {
            Some(f) => f,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS), // EBADF
        };

        match file_desc.kind {
            FdKind::Stdin => {
                // Keyboard input no longer comes through the kernel: the
                // PS/2 driver is a userspace program, and init forwards its
                // keys (and the kernel's USB keys) to the shell over a
                // channel. There is no terminal behind fd 0.
                let _ = (len, ptr);
                return err_to_ret(RxStatus::ERR_NOT_SUPPORTED);
            }
            FdKind::File { inode, offset } => {
                // Get the ramdisk file info
                use crate::fs::ramdisk;
                let ramdisk = match ramdisk::get_ramdisk() {
                    Ok(r) => r,
                    Err(_) => return err_to_ret(RxStatus::ERR_NOT_FOUND),
                };

                // Get file headers array
                let files = unsafe {
                    let base = ramdisk.data.as_ptr().add(ramdisk.superblock.files_offset as usize);
                    let count = ramdisk.superblock.num_files as usize;
                    core::slice::from_raw_parts(base as *const ramdisk::RamdiskFile, count)
                };

                // Find the file by inode (index)
                let ramdisk_file = match files.get(inode as usize) {
                    Some(&f) => f,
                    None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
                };

                Some((ramdisk_file, offset, len, ptr))
            }
            _ => {
                // Stdout/stderr not readable
                return err_to_ret(RxStatus::ERR_INVALID_ARGS);
            }
        }
    };

    if let Some((ramdisk_file, offset, len, ptr)) = file_info {
        use crate::fs::ramdisk;
        let ramdisk = ramdisk::get_ramdisk().unwrap();

        // Calculate remaining bytes from current offset
        let file_size = ramdisk_file.size as u64;
        let remaining = if offset >= file_size {
            0
        } else {
            file_size - offset
        };

        if remaining == 0 {
            return ok_to_ret_isize(0); // EOF
        }

        let to_read = core::cmp::min(len as u64, remaining) as usize;

        // Read from the file at current offset
        let data_offset = ramdisk_file.data_offset as usize + offset as usize;
        let data_ptr = unsafe {
            ramdisk.data.as_ptr().add(data_offset)
        };

        let data = unsafe { core::slice::from_raw_parts(data_ptr, to_read) };
        if let Err(e) = user::write_bytes(ptr as usize, data) {
            return err_to_ret(e);
        }

        // Update offset in fd_table
        let mut table = PROCESS_TABLE.lock();
        if let Some(current) = table.current_mut() {
            if let Some(fd_entry) = current.fd_table.get_mut(fd) {
                if let FdKind::File { ref mut offset, .. } = fd_entry.kind {
                    *offset += to_read as u64;
                }
            }
        }

        ok_to_ret_isize(to_read as isize)
    } else {
        ok_to_ret_isize(0)
    }
}

/// Open a file from the ramdisk
///
/// Arguments:
///   arg0: pointer to path string (null-terminated, userspace)
///   arg1: flags (O_RDONLY, O_WRONLY, O_RDWR)
///
/// Returns: file descriptor number, or negative error code
///
/// Phase 5C: This opens files from the embedded ramdisk filesystem.
/// The path must be a null-terminated string in userspace memory.
fn sys_open(args: SyscallArgs) -> SyscallRet {
    use crate::fs::ramdisk::{self, Errno};
    use crate::syscall::fd::{FdKind, flags};
    use crate::process::table::PROCESS_TABLE;

    let path_ptr = args.arg_u64(0) as *const u8;
    let flags_val = args.arg_u32(1);

    // Validate path pointer
    if path_ptr.is_null() {
        return err_to_ret(RxStatus::ERR_INVALID_ARGS);
    }

    // Read null-terminated path string from userspace (max 256 bytes)
    let path_string = match user::read_cstr(path_ptr as usize, 256) {
        Ok(p) => p,
        Err(e) => return err_to_ret(e),
    };
    let path = path_string.as_str();

    // Look up file in ramdisk
    let ramdisk_file = {
        let ramdisk = match ramdisk::get_ramdisk() {
            Ok(r) => r,
            Err(e) => {
                // Convert Errno to RxStatus
                return err_to_ret(match e {
                    Errno::ENODEV => RxStatus::ERR_NOT_FOUND,
                    _ => RxStatus::ERR_INVALID_ARGS,
                });
            }
        };

        match ramdisk.find_file(path) {
            Some(f) => f,
            None => return err_to_ret(RxStatus::ERR_NOT_FOUND), // ENOENT
        }
    };

    // Get the current process and allocate fd
    let fd_result = {
        let mut table = PROCESS_TABLE.lock();
        let current = match table.current_mut() {
            Some(p) => p,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
        };

        // Find the inode (file index) for offset tracking
        let inode = {
            let ramdisk = match ramdisk::get_ramdisk() {
                Ok(r) => r,
                Err(_) => return err_to_ret(RxStatus::ERR_NOT_FOUND),
            };

            let files = unsafe {
                let base = ramdisk.data.as_ptr().add(ramdisk.superblock.files_offset as usize);
                let count = ramdisk.superblock.num_files as usize;
                core::slice::from_raw_parts(base as *const ramdisk::RamdiskFile, count)
            };

            // Find the index of this file
            files.iter().position(|&f| {
                f.data_offset == ramdisk_file.data_offset &&
                f.name_offset == ramdisk_file.name_offset
            }).unwrap_or(0) as u32
        };

        // Allocate file descriptor
        match current.fd_table.alloc(
            FdKind::File {
                inode,
                offset: 0,
            },
            flags_val,
        ) {
            Some(fd) => fd as usize,
            None => return err_to_ret(RxStatus::ERR_NO_MEMORY), // EMFILE
        }
    };

    ok_to_ret(fd_result)
}

/// Close a file descriptor
///
/// Arguments:
///   arg0: file descriptor (fd)
///
/// Returns: 0 on success, or negative error code
///
/// Phase 5C: This closes files and releases the file descriptor.
/// stdin/stdout/stderr cannot be closed.
fn sys_close(args: SyscallArgs) -> SyscallRet {
    use crate::process::table::PROCESS_TABLE;

    let fd = args.arg(0) as u8;

    let mut table = PROCESS_TABLE.lock();
    let current = match table.current_mut() {
        Some(p) => p,
        None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
    };

    match current.fd_table.close(fd) {
        Some(_) => ok_to_ret(0),
        None => err_to_ret(RxStatus::ERR_INVALID_ARGS), // EBADF
    }
}

/// Seek to a position in a file
///
/// Arguments:
///   arg0: file descriptor (fd)
///   arg1: offset in bytes
///   arg2: whence (0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END)
///
/// Returns: new file offset, or negative error code
///
/// Phase 5C: This changes the file offset for reads.
fn sys_lseek(args: SyscallArgs) -> SyscallRet {
    use crate::syscall::fd::FdKind;
    use crate::fs::ramdisk;
    use crate::process::table::PROCESS_TABLE;

    let fd = args.arg(0) as u8;
    let offset = args.arg_i64(1);
    let whence = args.arg(2) as u32;

    // Validate whence
    if whence > 2 {
        return err_to_ret(RxStatus::ERR_INVALID_ARGS);
    }

    // Get current offset and file info
    let (current_offset, file_size) = {
        let table = PROCESS_TABLE.lock();
        let current = match table.current() {
            Some(p) => p,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
        };

        let file_desc = match current.fd_table.get(fd) {
            Some(f) => f,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS), // EBADF
        };

        match file_desc.kind {
            FdKind::File { inode, offset } => {
                // Get file size from ramdisk
                let ramdisk = match ramdisk::get_ramdisk() {
                    Ok(r) => r,
                    Err(_) => return err_to_ret(RxStatus::ERR_NOT_FOUND),
                };

                let files = unsafe {
                    let base = ramdisk.data.as_ptr().add(ramdisk.superblock.files_offset as usize);
                    let count = ramdisk.superblock.num_files as usize;
                    core::slice::from_raw_parts(base as *const ramdisk::RamdiskFile, count)
                };

                let file = match files.get(inode as usize) {
                    Some(&f) => f,
                    None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
                };

                (offset, file.size as i64)
            }
            _ => {
                // Cannot seek on stdin/stdout/stderr
                return err_to_ret(RxStatus::ERR_INVALID_ARGS); // ESPIPE
            }
        }
    };

    // Calculate new offset
    let new_offset = match whence {
        0 => {
            // SEEK_SET
            if offset < 0 {
                return err_to_ret(RxStatus::ERR_INVALID_ARGS);
            }
            offset
        }
        1 => {
            // SEEK_CUR
            let cur = current_offset as i64;
            let new = cur + offset;
            if new < 0 {
                return err_to_ret(RxStatus::ERR_INVALID_ARGS);
            }
            new
        }
        2 => {
            // SEEK_END
            let new = file_size + offset;
            if new < 0 {
                return err_to_ret(RxStatus::ERR_INVALID_ARGS);
            }
            new
        }
        _ => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
    };

    // Clamp to file size
    let clamped_offset = if new_offset > file_size {
        file_size as u64
    } else {
        new_offset as u64
    };

    // Update offset in fd_table
    {
        let mut table = PROCESS_TABLE.lock();
        let current = match table.current_mut() {
            Some(p) => p,
            None => return err_to_ret(RxStatus::ERR_INVALID_ARGS),
        };

        if let Some(fd_entry) = current.fd_table.get_mut(fd) {
            if let FdKind::File { ref mut offset, .. } = fd_entry.kind {
                *offset = clamped_offset;
            }
        }
    }

    ok_to_ret_isize(clamped_offset as isize)
}

// ============================================================================
// Process Info Syscalls (Phase 5A)
// ============================================================================

/// Get current process ID
///
/// Arguments: none
///
/// Returns: process ID (PID)
///
/// Returns the PID of the currently running process.
fn sys_getpid(_args: SyscallArgs) -> SyscallRet {
    use crate::sched::round_robin;

    match round_robin::get_current_pid() {
        Some(pid) => ok_to_ret(pid as usize),
        None => {
            // No current process - return kernel PID (0)
            ok_to_ret(0)
        }
    }
}

/// Get parent process ID
///
/// Arguments: none
///
/// Returns: parent process ID (PPID)
///
/// Returns the PPID of the currently running process.
fn sys_getppid(_args: SyscallArgs) -> SyscallRet {
    use crate::sched::round_robin;

    match round_robin::get_current_ppid() {
        Some(ppid) => ok_to_ret(ppid as usize),
        None => {
            // No current process - return kernel PPID (0)
            ok_to_ret(0)
        }
    }
}

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
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
                }
            }
            for b in e.as_bytes() {
                unsafe {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") *b, options(nomem, nostack));
                }
            }
            let msg = b"\n";
            for &b in msg {
                unsafe {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack));
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
    pub const SPAWN: u32 = 0x03;  // Spawn process from ramdisk path
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

    /// I/O (0x60-0x6F) - Phase 5A
    pub const WRITE: u32 = 0x60;
    pub const READ: u32 = 0x61;
    pub const OPEN: u32 = 0x62;
    pub const CLOSE: u32 = 0x63;
    pub const LSEEK: u32 = 0x64;

    /// Process Info (0x70-0x7F) - Phase 5A
    pub const GETPID: u32 = 0x70;
    pub const GETPPID: u32 = 0x71;
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
