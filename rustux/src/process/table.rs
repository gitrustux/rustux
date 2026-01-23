// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Process Table
//!
//! This module provides the global process table for tracking all processes
//! in the system. It implements the Phase 5B requirements for process
//! management and context switching.

use crate::arch::amd64::mm::page_tables::PAddr;
use crate::syscall::fd::FileDescriptorTable;
use crate::sync::SpinMutex;

/// ============================================================================
/// Process State
/// ============================================================================

/// Process state for Phase 5B scheduler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Process is ready to run
    Ready,
    /// Process is currently running
    Running,
    /// Process is blocked (waiting for I/O, event, etc.)
    Blocked,
    /// Process has exited but not yet reaped by parent
    Zombie,
    /// Process is dead (resources freed)
    Dead,
}

impl ProcessState {
    /// Check if process is runnable
    pub const fn is_runnable(&self) -> bool {
        matches!(self, Self::Ready | Self::Running)
    }

    /// Check if process is alive
    pub const fn is_alive(&self) -> bool {
        matches!(self, Self::Ready | Self::Running | Self::Blocked)
    }
}

/// ============================================================================
/// Saved CPU State
/// ============================================================================

/// Saved CPU state during context switch (Phase 5B)
///
/// This structure contains all the CPU state that needs to be saved
/// and restored during a context switch. It's designed to match the
/// layout expected by the context_switch assembly function.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SavedState {
    // General-purpose registers
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8:  u64,
    pub r9:  u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,

    // Control registers
    pub cr3: u64,
    pub rflags: u64,

    // Instruction pointer
    pub rip: u64,

    // Segment selectors
    pub cs: u64,
    pub ss: u64,

    // FPU state (512 bytes for FXSAVE)
    #[doc(hidden)]
    pub fpu: [u8; 512],
}

impl SavedState {
    /// Create a new zeroed SavedState
    pub const fn new() -> Self {
        Self {
            rax: 0, rbx: 0, rcx: 0, rdx: 0,
            rsi: 0, rdi: 0, rbp: 0, rsp: 0,
            r8: 0, r9: 0, r10: 0, r11: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
            cr3: 0,
            rflags: 0,
            rip: 0,
            cs: 0,
            ss: 0,
            fpu: [0; 512],
        }
    }

    /// Create a SavedState for a new userspace process
    ///
    /// # Arguments
    ///
    /// * `entry` - Entry point address (RIP)
    /// * `user_stack_top` - Top of user stack (RSP)
    /// * `cr3` - Page table physical address
    pub fn for_userspace(entry: u64, user_stack_top: u64, cr3: u64) -> Self {
        Self {
            rax: 0, rbx: 0, rcx: 0, rdx: 0,
            rsi: 0, rdi: 0, rbp: 0, rsp: user_stack_top,
            r8: 0, r9: 0, r10: 0, r11: 0,
            r12: 0, r13: 0, r14: 0, r15: 0,
            cr3,
            rflags: 0x202, // IF=1 (interrupts enabled)
            rip: entry,
            cs: 0x1B,      // User code segment (RPL=3)
            ss: 0x23,      // User data segment (RPL=3)
            fpu: [0; 512],
        }
    }

    /// Create a SavedState for returning from a syscall
    ///
    /// This is used when a process makes a syscall and needs to
    /// return to userspace with a return value.
    pub fn for_syscall_return(&self, ret_value: u64) -> Self {
        let mut state = *self;
        state.rax = ret_value; // Return value in RAX
        state
    }
}

impl Default for SavedState {
    fn default() -> Self {
        Self::new()
    }
}

/// ============================================================================
/// Process Descriptor (Phase 5B)
/// ============================================================================

/// Maximum number of processes in the system
const MAX_PROCESSES: usize = 256;

/// Process descriptor (Phase 5B)
///
/// This represents a process in the system with all the state needed
/// for scheduling and context switching.
pub struct Process {
    /// Process ID
    pub pid: u32,

    /// Parent process ID
    pub ppid: u32,

    /// Process state
    pub state: ProcessState,

    /// Physical address of page table (CR3 value)
    pub page_table: PAddr,

    /// Kernel stack base (virtual address)
    pub kernel_stack: u64,

    /// User stack top (virtual address)
    pub user_stack: u64,

    /// Saved CPU state
    pub saved_state: SavedState,

    /// Syscall return value
    pub syscall_ret: u64,

    /// File descriptor table
    pub fd_table: FileDescriptorTable,

    /// Time accounting
    pub cpu_time: u64,
    pub sched_time: u64,

    /// Process name (for debugging)
    pub name: Option<alloc::string::String>,
}

impl Process {
    /// Create a new process
    ///
    /// # Arguments
    ///
    /// * `pid` - Process ID
    /// * `ppid` - Parent process ID
    /// * `page_table` - Physical address of page table
    /// * `kernel_stack` - Kernel stack base (virtual address)
    /// * `user_stack` - User stack top (virtual address)
    /// * `entry` - Entry point address
    pub fn new(
        pid: u32,
        ppid: u32,
        page_table: PAddr,
        kernel_stack: u64,
        user_stack: u64,
        entry: u64,
    ) -> Self {
        let mut fd_table = FileDescriptorTable::new();
        fd_table.init();

        Self {
            pid,
            ppid,
            state: ProcessState::Ready,
            page_table,
            kernel_stack,
            user_stack,
            saved_state: SavedState::for_userspace(entry, user_stack, page_table),
            syscall_ret: 0,
            fd_table,
            cpu_time: 0,
            sched_time: 0,
            name: None,
        }
    }

    /// Set the process name
    pub fn set_name(&mut self, name: alloc::string::String) {
        self.name = Some(name);
    }

    /// Get the process name
    pub fn get_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// ============================================================================
/// Process Table
/// ============================================================================

/// Global process table
///
/// This table tracks all processes in the system and provides
/// methods for lookup, insertion, and management.
pub struct ProcessTable {
    /// Process array (indexed by PID)
    processes: [Option<Process>; MAX_PROCESSES],

    /// Current running process
    current: Option<u32>,

    /// Next PID to allocate
    next_pid: u32,
}

impl ProcessTable {
    /// Create a new process table
    pub const fn new() -> Self {
        const NONE: Option<Process> = None;
        Self {
            processes: [NONE; MAX_PROCESSES],
            current: None,
            next_pid: 1, // PID 0 is kernel
        }
    }

    /// Get the current process
    pub fn current(&self) -> Option<&Process> {
        self.current.and_then(|pid| self.processes.get(pid as usize)?.as_ref())
    }

    /// Get the current process (mutable)
    pub fn current_mut(&mut self) -> Option<&mut Process> {
        let pid = self.current?;
        self.processes.get_mut(pid as usize)?.as_mut()
    }

    /// Get a process by PID
    pub fn get(&self, pid: u32) -> Option<&Process> {
        self.processes.get(pid as usize)?.as_ref()
    }

    /// Get a process by PID (mutable)
    pub fn get_mut(&mut self, pid: u32) -> Option<&mut Process> {
        self.processes.get_mut(pid as usize)?.as_mut()
    }

    /// Allocate a new PID
    pub fn alloc_pid(&mut self) -> Option<u32> {
        // Find next free PID
        let start = self.next_pid;
        loop {
            if self.next_pid >= MAX_PROCESSES as u32 {
                return None; // No more PIDs available
            }

            let pid = self.next_pid;
            self.next_pid += 1;

            if self.processes[pid as usize].is_none() {
                return Some(pid);
            }

            // Wrapped around
            if self.next_pid == start {
                return None;
            }
        }
    }

    /// Insert a process into the table
    ///
    /// # Panics
    ///
    /// Panics if the PID is already in use or out of range
    pub fn insert(&mut self, process: Process) {
        let pid = process.pid;
        if (pid as usize) >= MAX_PROCESSES {
            panic!("PID out of range: {}", pid);
        }
        if self.processes[pid as usize].is_some() {
            panic!("PID already in use: {}", pid);
        }
        self.processes[pid as usize] = Some(process);
    }

    /// Set the current running process
    pub fn set_current(&mut self, pid: u32) {
        self.current = Some(pid);
    }

    /// Get the current PID
    pub fn current_pid(&self) -> Option<u32> {
        self.current
    }

    /// Remove a process from the table
    pub fn remove(&mut self, pid: u32) -> Option<Process> {
        if pid >= MAX_PROCESSES as u32 {
            return None;
        }

        // If this is the current process, clear current
        if self.current == Some(pid) {
            self.current = None;
        }

        self.processes[pid as usize].take()
    }

    /// Find the next runnable process
    pub fn find_next_runnable(&self, current_pid: Option<u32>) -> Option<u32> {
        // Start from the process after current (or 0 if none)
        let start = current_pid.map_or(0, |p| (p + 1) % MAX_PROCESSES as u32);

        // Search for a runnable process
        let mut pid = start;
        loop {
            if let Some(process) = self.get(pid) {
                if process.state.is_runnable() {
                    return Some(pid);
                }
            }

            pid = (pid + 1) % MAX_PROCESSES as u32;

            if pid == start {
                // Wrapped around, no runnable process
                return None;
            }
        }
    }

    /// Get all runnable PIDs
    pub fn runnable_pids(&self) -> alloc::vec::Vec<u32> {
        let mut pids = alloc::vec::Vec::new();
        for (pid, process) in self.processes.iter().enumerate() {
            if let Some(p) = process {
                if p.state.is_runnable() {
                    pids.push(pid as u32);
                }
            }
        }
        pids
    }

    /// Get process count
    pub fn count(&self) -> usize {
        self.processes.iter().filter(|p| p.is_some()).count()
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

/// ============================================================================
/// Global Process Table
/// ============================================================================

/// Global process table instance
pub static PROCESS_TABLE: SpinMutex<ProcessTable> = SpinMutex::new(ProcessTable::new());

/// ============================================================================
/// Helper type for SpinMutex guard
/// ============================================================================

// Re-export for convenience
pub use crate::sync::SpinMutexGuard;

/// Get a reference to the current process with manual locking
///
/// This is the preferred way to access the current process.
/// The caller must manage the lock lifetime carefully.
pub fn with_current_process<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Process) -> R,
{
    let table = PROCESS_TABLE.lock();
    let current = table.current_pid()?;
    let process = table.get(current)?;
    Some(f(process))
}

/// Get a mutable reference to the current process with manual locking
///
/// This is the preferred way to modify the current process.
/// The caller must manage the lock lifetime carefully.
pub fn with_current_process_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Process) -> R,
{
    let mut table = PROCESS_TABLE.lock();
    let current = table.current?;
    let process = table.get_mut(current)?;
    Some(f(process))
}

/// Get a process by PID with manual locking
pub fn with_process<F, R>(pid: u32, f: F) -> Option<R>
where
    F: FnOnce(&Process) -> R,
{
    let table = PROCESS_TABLE.lock();
    let process = table.get(pid)?;
    Some(f(process))
}

/// Get a mutable process by PID with manual locking
pub fn with_process_mut<F, R>(pid: u32, f: F) -> Option<R>
where
    F: FnOnce(&mut Process) -> R,
{
    let mut table = PROCESS_TABLE.lock();
    let process = table.get_mut(pid)?;
    Some(f(process))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saved_state_new() {
        let state = SavedState::new();
        assert_eq!(state.rax, 0);
        assert_eq!(state.rip, 0);
        assert_eq!(state.rsp, 0);
    }

    #[test]
    fn test_saved_state_for_userspace() {
        let state = SavedState::for_userspace(0x1000, 0x7000_0000_0000, 0x5000);
        assert_eq!(state.rip, 0x1000);
        assert_eq!(state.rsp, 0x7000_0000_0000);
        assert_eq!(state.cr3, 0x5000);
        assert_eq!(state.cs, 0x1B);
        assert_eq!(state.ss, 0x23);
        assert_eq!(state.rflags, 0x202);
    }

    #[test]
    fn test_process_state() {
        assert!(ProcessState::Ready.is_runnable());
        assert!(ProcessState::Running.is_runnable());
        assert!(!ProcessState::Blocked.is_runnable());
        assert!(!ProcessState::Zombie.is_runnable());
        assert!(!ProcessState::Dead.is_runnable());
    }

    #[test]
    fn test_process_table_new() {
        let table = ProcessTable::new();
        assert!(table.current().is_none());
        assert_eq!(table.next_pid, 1);
        assert_eq!(table.count(), 0);
    }

    #[test]
    fn test_process_table_alloc_pid() {
        let mut table = ProcessTable::new();
        assert_eq!(table.alloc_pid(), Some(1));
        assert_eq!(table.alloc_pid(), Some(2));
        assert_eq!(table.next_pid, 3);
    }

    #[test]
    fn test_process_table_insert_get() {
        let mut table = ProcessTable::new();
        let process = Process::new(1, 0, PhysAddr::new(0x1000), 0x2000, 0x7000_0000_0000, 0x4000);

        table.insert(process);
        assert_eq!(table.count(), 1);

        let retrieved = table.get(1);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().pid, 1);
    }

    #[test]
    fn test_process_table_current() {
        let mut table = ProcessTable::new();
        let process = Process::new(1, 0, PhysAddr::new(0x1000), 0x2000, 0x7000_0000_0000, 0x4000);

        table.insert(process);
        table.set_current(1);

        assert_eq!(table.current_pid(), Some(1));
        assert_eq!(table.current().unwrap().pid, 1);
    }

    #[test]
    fn test_process_table_find_next_runnable() {
        let mut table = ProcessTable::new();

        // Add some processes
        let p1 = Process::new(1, 0, PhysAddr::new(0x1000), 0x2000, 0x7000_0000_0000, 0x4000);
        let p2 = Process::new(2, 1, PhysAddr::new(0x5000), 0x6000, 0x7000_0000_0000, 0x7000);
        let p3 = Process::new(3, 1, PhysAddr::new(0x9000), 0xA000, 0x7000_0000_0000, 0xB000);

        table.insert(p1);
        table.insert(p2);
        table.insert(p3);

        // All should be runnable (state=Ready)
        assert_eq!(table.find_next_runnable(None), Some(1));
        assert_eq!(table.find_next_runnable(Some(1)), Some(2));
        assert_eq!(table.find_next_runnable(Some(2)), Some(3));
        assert_eq!(table.find_next_runnable(Some(3)), Some(1)); // Wrap around
    }
}
