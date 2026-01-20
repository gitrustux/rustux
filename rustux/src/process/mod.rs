// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Process Management
//!
//! This module provides process management for the Rustux kernel.
//! Processes represent isolated execution contexts with their own address spaces.
//!
//! # Design
//!
//! - Each process has a unique process ID (PID)
//! - Processes have address spaces (VM mappings)
//! - Processes contain threads
//! - Processes have handle tables for capability-based security
//! - Processes are organized in a hierarchy with parent/child relationships
//! - Processes belong to jobs for resource accounting
//!
//! # Process States
//!
//! ```text
//! Creating -> Running -> Exiting -> Dead
//! ```

pub mod address_space;

use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::SpinMutex;

/// ============================================================================
/// Process ID
/// ============================================================================

/// Process ID type
pub type ProcessId = u64;

/// Invalid process ID
pub const PID_INVALID: ProcessId = 0;

/// Kernel process ID (PID 0)
pub const PID_KERNEL: ProcessId = 0;

/// First user process ID
pub const PID_FIRST_USER: ProcessId = 1;

/// Global process ID allocator
static PID_ALLOCATOR: PidAllocator = PidAllocator::new();

/// Process ID allocator
struct PidAllocator {
    next: AtomicU64,
}

impl PidAllocator {
    const fn new() -> Self {
        Self {
            next: AtomicU64::new(PID_FIRST_USER), // Start at 1
        }
    }

    fn allocate(&self) -> ProcessId {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}

/// ============================================================================
/// Process State
/// ============================================================================

/// Process state
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is being created
    Creating = 0,

    /// Process is running (has at least one thread)
    Running = 1,

    /// Process is exiting (threads terminating)
    Exiting = 2,

    /// Process is dead (all threads terminated, resources freed)
    Dead = 3,
}

impl ProcessState {
    /// Check if process is alive
    pub const fn is_alive(self) -> bool {
        matches!(self, Self::Creating | Self::Running | Self::Exiting)
    }

    /// Check if process has exited
    pub const fn has_exited(self) -> bool {
        matches!(self, Self::Exiting | Self::Dead)
    }
}

/// ============================================================================
/// Job ID
/// ============================================================================

/// Job ID type
///
/// Jobs are containers for processes that provide resource accounting.
pub type JobId = u64;

/// Invalid job ID
pub const JOB_ID_INVALID: JobId = 0;

/// Root job ID
pub const JOB_ID_ROOT: JobId = 1;

/// ============================================================================
/// Handle
/// ============================================================================

/// Handle type
///
/// Handles are capabilities that reference kernel objects.
pub type Handle = u32;

/// Invalid handle
pub const HANDLE_INVALID: Handle = 0;

/// Handle rights
///
/// Rights control what operations can be performed on an object.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleRights {
    /// None
    None = 0,

    /// Read
    Read = 1 << 0,

    /// Write
    Write = 1 << 1,

    /// Execute
    Execute = 1 << 2,

    /// Duplicate
    Duplicate = 1 << 3,

    /// Transfer
    Transfer = 1 << 4,

    /// All rights
    All = 0xFFFF_FFFF,
}

impl HandleRights {
    /// Check if has right
    pub const fn has(self, right: Self) -> bool {
        (self as u32) & (right as u32) != 0
    }

    /// Add a right
    pub const fn add(self, right: Self) -> Self {
        unsafe { core::mem::transmute((self as u32) | (right as u32)) }
    }

    /// Remove a right
    pub const fn remove(self, right: Self) -> Self {
        unsafe { core::mem::transmute((self as u32) & !(right as u32)) }
    }
}

/// ============================================================================
/// Object Type
/// ============================================================================

/// Object type
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    /// Process
    Process = 1,

    /// Thread
    Thread = 2,

    /// VMO (Virtual Memory Object)
    Vmo = 3,

    /// VMAR (Virtual Memory Address Region)
    Vmar = 4,

    /// Channel
    Channel = 5,

    /// Event
    Event = 6,

    /// Event pair
    EventPair = 7,

    /// Job
    Job = 8,

    /// Timer
    Timer = 9,

    /// Unknown
    Unknown = 0xFFFF,
}

/// ============================================================================
/// Handle Table Entry
/// ============================================================================

/// Handle table entry
#[repr(C)]
#[derive(Debug, Clone)]
pub struct HandleEntry {
    /// Handle value
    pub handle: Handle,

    /// Object ID (what the handle refers to)
    pub object_id: u64,

    /// Handle rights
    pub rights: HandleRights,

    /// Type of object
    pub object_type: ObjectType,
}

/// ============================================================================
/// Handle Table
/// ============================================================================

/// Maximum handles per process
pub const MAX_HANDLES: usize = 256;

/// Handle table
///
/// Manages handles for a single process.
pub struct HandleTable {
    /// Handle entries
    handles: [Option<HandleEntry>; MAX_HANDLES],

    /// Next handle index to allocate
    next_index: core::sync::atomic::AtomicUsize,

    /// Number of active handles
    count: core::sync::atomic::AtomicUsize,
}

impl HandleTable {
    /// Create a new handle table
    pub const fn new() -> Self {
        const NONE: Option<HandleEntry> = None;
        Self {
            handles: [NONE; MAX_HANDLES],
            next_index: core::sync::atomic::AtomicUsize::new(0),
            count: core::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Allocate a new handle
    pub fn alloc(&mut self, object_id: u64, rights: HandleRights, object_type: ObjectType) -> Result<Handle, &'static str> {
        // Find a free slot
        let start = self.next_index.load(Ordering::Relaxed);
        let mut idx = start;

        for _ in 0..MAX_HANDLES {
            if self.handles[idx].is_none() {
                let handle = (idx + 1) as u32; // Handles start at 1

                self.handles[idx] = Some(HandleEntry {
                    handle,
                    object_id,
                    rights,
                    object_type,
                });

                self.next_index.store((idx + 1) % MAX_HANDLES, Ordering::Relaxed);
                self.count.fetch_add(1, Ordering::Relaxed);

                return Ok(handle);
            }

            idx = (idx + 1) % MAX_HANDLES;
            if idx == start {
                break; // Wrapped around, table is full
            }
        }

        Err("handle table full")
    }

    /// Free a handle
    pub fn free(&mut self, handle: Handle) -> Result<(), &'static str> {
        let idx = (handle as usize) - 1; // Handles start at 1

        if idx >= MAX_HANDLES {
            return Err("invalid handle");
        }

        if self.handles[idx].is_none() {
            return Err("handle not allocated");
        }

        self.handles[idx] = None;
        self.count.fetch_sub(1, Ordering::Relaxed);

        Ok(())
    }

    /// Get a handle entry
    pub fn get(&self, handle: Handle) -> Result<&HandleEntry, &'static str> {
        let idx = (handle as usize) - 1; // Handles start at 1

        if idx >= MAX_HANDLES {
            return Err("invalid handle");
        }

        self.handles[idx].as_ref().ok_or("handle not allocated")
    }

    /// Get a mutable handle entry
    pub fn get_mut(&mut self, handle: Handle) -> Result<&mut HandleEntry, &'static str> {
        let idx = (handle as usize) - 1; // Handles start at 1

        if idx >= MAX_HANDLES {
            return Err("invalid handle");
        }

        self.handles[idx].as_mut().ok_or("handle not allocated")
    }

    /// Check if a handle has specific rights
    pub fn check_rights(&self, handle: Handle, rights: HandleRights) -> Result<(), &'static str> {
        let entry = self.get(handle)?;

        if !entry.rights.has(rights) {
            return Err("insufficient rights");
        }

        Ok(())
    }

    /// Get the number of active handles
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

/// ============================================================================
/// Process Structure
/// ============================================================================

/// Maximum number of threads per process
pub const MAX_THREADS_PER_PROCESS: usize = 1024;

// Re-export the actual AddressSpace implementation
pub use address_space::AddressSpace;

/// Process flags
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessFlags {
    /// None
    None = 0,

    /// Created with loader stub
    Loader = 1 << 0,

    /// Created for testing
    Test = 1 << 1,

    /// Created as system process
    System = 1 << 2,
}

impl ProcessFlags {
    /// Check if flag is set
    pub const fn has(self, flag: Self) -> bool {
        (self as u32) & (flag as u32) != 0
    }

    /// Add a flag
    pub const fn add(self, flag: Self) -> Self {
        unsafe { core::mem::transmute((self as u32) | (flag as u32)) }
    }
}

/// Return code type
pub type ReturnCode = i32;

/// Process structure
///
/// Represents a process in the system.
pub struct Process {
    /// Process ID
    pub pid: ProcessId,

    /// Process state
    pub state: SpinMutex<ProcessState>,

    /// Address space (placeholder until VMM is migrated)
    pub address_space: SpinMutex<Option<&'static mut AddressSpace>>,

    /// Handle table
    pub handles: SpinMutex<HandleTable>,

    /// Threads in this process (thread IDs)
    pub threads: SpinMutex<alloc::vec::Vec<u64>>,

    /// Parent process ID
    pub parent_pid: SpinMutex<Option<ProcessId>>,

    /// Job ID
    pub job_id: JobId,

    /// Return code (when process exits)
    pub return_code: SpinMutex<Option<ReturnCode>>,

    /// Process name (for debugging)
    pub name: SpinMutex<Option<&'static str>>,

    /// Reference count
    pub ref_count: AtomicU64,

    /// Creation flags
    pub flags: ProcessFlags,
}

impl Process {
    /// Create a new process
    pub fn new(parent_pid: Option<ProcessId>, job_id: JobId, flags: ProcessFlags) -> alloc::boxed::Box<Self> {
        let pid = PID_ALLOCATOR.allocate();

        alloc::boxed::Box::new(Self {
            pid,
            state: SpinMutex::new(ProcessState::Creating),
            address_space: SpinMutex::new(None),
            handles: SpinMutex::new(HandleTable::new()),
            threads: SpinMutex::new(alloc::vec::Vec::new()),
            parent_pid: SpinMutex::new(parent_pid),
            job_id,
            return_code: SpinMutex::new(None),
            name: SpinMutex::new(None),
            ref_count: AtomicU64::new(1),
            flags,
        })
    }

    /// Get the process ID
    pub fn pid(&self) -> ProcessId {
        self.pid
    }

    /// Get the process state
    pub fn get_state(&self) -> ProcessState {
        *self.state.lock()
    }

    /// Set the process state
    pub fn set_state(&self, new_state: ProcessState) {
        *self.state.lock() = new_state;
    }

    /// Add a thread to the process
    pub fn add_thread(&self, tid: u64) -> Result<(), &'static str> {
        let mut threads = self.threads.lock();

        if threads.len() >= MAX_THREADS_PER_PROCESS {
            return Err("too many threads");
        }

        threads.push(tid);
        Ok(())
    }

    /// Remove a thread from the process
    pub fn remove_thread(&self, tid: u64) {
        let mut threads = self.threads.lock();
        if let Some(pos) = threads.iter().position(|&t| t == tid) {
            threads.remove(pos);
        }
    }

    /// Get the number of threads
    pub fn thread_count(&self) -> usize {
        self.threads.lock().len()
    }

    /// Get the parent process ID
    pub fn get_parent_pid(&self) -> Option<ProcessId> {
        *self.parent_pid.lock()
    }

    /// Exit the process
    pub fn exit(&self, code: ReturnCode) {
        // Set return code
        *self.return_code.lock() = Some(code);

        // Transition to exiting state
        self.set_state(ProcessState::Exiting);
    }

    /// Set the process name
    pub fn set_name(&self, name: &'static str) {
        *self.name.lock() = Some(name);
    }

    /// Get the process name
    pub fn get_name(&self) -> Option<&'static str> {
        *self.name.lock()
    }

    /// Increment reference count
    pub fn ref_inc(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement reference count
    ///
    /// Returns true if this was the last reference
    pub fn ref_dec(&self) -> bool {
        self.ref_count.fetch_sub(1, Ordering::Relaxed) == 1
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_allocator() {
        let pid1 = PID_ALLOCATOR.allocate();
        let pid2 = PID_ALLOCATOR.allocate();

        assert!(pid1 < pid2);
        assert_eq!(pid1, PID_FIRST_USER);
    }

    #[test]
    fn test_process_state() {
        assert!(ProcessState::Creating.is_alive());
        assert!(ProcessState::Running.is_alive());
        assert!(!ProcessState::Dead.is_alive());

        assert!(ProcessState::Exiting.has_exited());
        assert!(ProcessState::Dead.has_exited());
        assert!(!ProcessState::Creating.has_exited());
    }

    #[test]
    fn test_handle_rights() {
        let rights = HandleRights::Read;

        assert!(rights.has(HandleRights::Read));
        assert!(!rights.has(HandleRights::Write));

        let combined = rights.add(HandleRights::Write);
        assert!(combined.has(HandleRights::Read));
        assert!(combined.has(HandleRights::Write));

        let removed = combined.remove(HandleRights::Read);
        assert!(!removed.has(HandleRights::Read));
        assert!(removed.has(HandleRights::Write));
    }

    #[test]
    fn test_handle_table() {
        let mut table = HandleTable::new();

        // Allocate a handle
        let handle = table.alloc(123, HandleRights::Read, ObjectType::Process).unwrap();
        assert_eq!(handle, 1);
        assert_eq!(table.count(), 1);

        // Get the handle
        let entry = table.get(handle).unwrap();
        assert_eq!(entry.object_id, 123);
        assert_eq!(entry.object_type, ObjectType::Process);

        // Check rights
        assert!(table.check_rights(handle, HandleRights::Read).is_ok());
        assert!(table.check_rights(handle, HandleRights::Write).is_err());

        // Free the handle
        table.free(handle).unwrap();
        assert_eq!(table.count(), 0);
        assert!(table.get(handle).is_err());
    }

    #[test]
    fn test_process_basic() {
        let process = Process::new(Some(1), JOB_ID_ROOT, ProcessFlags::None);

        assert_eq!(process.pid(), PID_FIRST_USER);
        assert_eq!(process.get_state(), ProcessState::Creating);
        assert_eq!(process.get_parent_pid(), Some(1));

        process.add_thread(10).unwrap();
        assert_eq!(process.thread_count(), 1);

        process.remove_thread(10);
        assert_eq!(process.thread_count(), 0);
    }

    #[test]
    fn test_process_flags() {
        let flags = ProcessFlags::Loader;
        assert!(flags.has(ProcessFlags::Loader));

        let combined = flags.add(ProcessFlags::System);
        assert!(combined.has(ProcessFlags::Loader));
        assert!(combined.has(ProcessFlags::System));
    }
}
