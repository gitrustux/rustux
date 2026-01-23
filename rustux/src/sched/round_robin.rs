// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Round-Robin Scheduler (Phase 5B)
//!
//! This module provides a simple round-robin scheduler that works with
//! the process table to schedule multiple processes. It implements the
//! Phase 5B requirements for timer-based scheduling and context switching.

use crate::process::table::{Process, ProcessState, ProcessTable, PROCESS_TABLE};
use crate::process::switch;
use crate::sync::SpinMutex;

/// Default time slice in milliseconds
pub const DEFAULT_TIME_SLICE_MS: u64 = 10;

/// ============================================================================
/// Round-Robin Scheduler
/// ============================================================================

/// Round-robin scheduler for Phase 5B
///
/// This scheduler implements simple round-robin scheduling with:
/// - Fixed time slices for each process
/// - Timer-based preemption
/// - Voluntary yielding via sys_yield
pub struct RoundRobinScheduler {
    /// Currently running process
    current: Option<u32>,

    /// Time slice in milliseconds
    time_slice_ms: u64,

    /// Preemption enabled
    preemption_enabled: bool,
}

impl RoundRobinScheduler {
    /// Create a new round-robin scheduler
    pub const fn new() -> Self {
        Self {
            current: None,
            time_slice_ms: DEFAULT_TIME_SLICE_MS,
            preemption_enabled: true,
        }
    }

    /// Get the current process PID
    pub fn current(&self) -> Option<u32> {
        self.current
    }

    /// Set the current process PID
    pub fn set_current(&mut self, pid: u32) {
        self.current = Some(pid);
    }

    /// Get the time slice in milliseconds
    pub fn time_slice_ms(&self) -> u64 {
        self.time_slice_ms
    }

    /// Set the time slice in milliseconds
    pub fn set_time_slice_ms(&mut self, ms: u64) {
        self.time_slice_ms = ms;
    }

    /// Check if preemption is enabled
    pub fn is_preemption_enabled(&self) -> bool {
        self.preemption_enabled
    }

    /// Enable or disable preemption
    pub fn set_preemption_enabled(&mut self, enabled: bool) {
        self.preemption_enabled = enabled;
    }

    /// Schedule the next process to run
    ///
    /// This function implements the core round-robin scheduling algorithm:
    /// 1. Mark the current process as Ready (if it was Running)
    /// 2. Find the next runnable process
    /// 3. Mark the next process as Running
    /// 4. Return the next process PID
    ///
    /// # Arguments
    ///
    /// * `process_table` - Mutable reference to the process table
    ///
    /// # Returns
    ///
    /// The PID of the next process to run, or None if no runnable process
    pub fn schedule(&mut self, process_table: &mut ProcessTable) -> Option<u32> {
        // Mark current as Ready if it was Running
        if let Some(current_pid) = self.current {
            if let Some(process) = process_table.get_mut(current_pid) {
                if process.state == ProcessState::Running {
                    process.state = ProcessState::Ready;
                }
            }
        }

        // Find next runnable process
        let next_pid = process_table.find_next_runnable(self.current);

        if let Some(pid) = next_pid {
            self.current = Some(pid);
            process_table.set_current(pid);

            if let Some(process) = process_table.get_mut(pid) {
                process.state = ProcessState::Running;
            }
        }

        next_pid
    }

    /// Perform a context switch to the next process
    ///
    /// This function:
    /// 1. Calls schedule() to find the next process
    /// 2. If the next process is different from current, performs context switch
    ///
    /// # Arguments
    ///
    /// * `process_table` - Mutable reference to the process table
    ///
    /// # Safety
    ///
    /// This function performs an unsafe context switch. The caller must ensure
    /// that the process table is properly locked and that both processes are valid.
    pub unsafe fn context_switch(&mut self, process_table: &mut ProcessTable) {
        let next_pid = self.schedule(process_table);

        if let Some(next_pid) = next_pid {
            if let Some(current_pid) = self.current {
                if current_pid != next_pid {
                    // We need to extract the data we need before the mutable borrow
                    // This is a simplified approach - in a real kernel we'd have
                    // more sophisticated locking
                    let next_cr3 = process_table.get(next_pid)
                        .map(|p| p.page_table)
                        .unwrap_or(0);

                    // Update current process state before switch
                    if let Some(process) = process_table.get_mut(current_pid) {
                        process.state = crate::process::table::ProcessState::Ready;
                    }

                    // Get pointers after the mutable borrow ends
                    let next_saved_ptr: *const crate::process::table::SavedState =
                        process_table.get(next_pid)
                            .map(|p| &p.saved_state as *const _)
                            .unwrap_or(core::ptr::null());

                    let current_saved_ptr: *mut crate::process::table::SavedState =
                        process_table.get_mut(current_pid)
                            .map(|p| &mut p.saved_state as *mut _)
                            .unwrap_or(core::ptr::null_mut());

                    // Perform the context switch using raw pointers
                    if !current_saved_ptr.is_null() && !next_saved_ptr.is_null() {
                        // Call the assembly function directly
                        crate::process::switch::context_switch_raw(
                            current_saved_ptr,
                            next_saved_ptr,
                            next_cr3,
                        );
                    }
                }
            }
        }
    }
}

impl Default for RoundRobinScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// ============================================================================
/// Global Scheduler Instance
/// ============================================================================

/// Global round-robin scheduler instance
pub static SCHEDULER: SpinMutex<RoundRobinScheduler> = SpinMutex::new(RoundRobinScheduler::new());

/// ============================================================================
/// Scheduler API Functions
/// ============================================================================

/// Timer tick handler
///
/// This function is called by the timer interrupt handler to implement
/// time-slice based preemption. It schedules the next process and may
/// perform a context switch.
///
/// # Usage
///
/// ```ignore
/// // In timer interrupt handler
/// unsafe {
///     sched::round_robin::timer_tick();
/// }
/// ```
pub unsafe fn timer_tick() {
    if !SCHEDULER.lock().is_preemption_enabled() {
        return;
    }

    let mut scheduler = SCHEDULER.lock();
    let mut process_table = PROCESS_TABLE.lock();

    scheduler.context_switch(&mut process_table);
}

/// Yield the CPU to another process
///
/// This function is called by the sys_yield syscall to voluntarily
/// give up the CPU to another process.
///
/// # Returns
///
/// * `Ok(())` - Successfully yielded
/// * `Err(&str)` - Failed to yield (no current process, etc.)
pub fn yield_cpu() -> Result<(), &'static str> {
    let mut scheduler = SCHEDULER.lock();
    let mut process_table = PROCESS_TABLE.lock();

    // Get current process
    let current_pid = scheduler.current().ok_or("No current process")?;

    // Check if there's another runnable process
    let next_pid = process_table.find_next_runnable(Some(current_pid));

    if let Some(next_pid) = next_pid {
        if next_pid != current_pid {
            unsafe {
                scheduler.context_switch(&mut process_table);
            }
        }
    }

    Ok(())
}

/// Get the current process PID
///
/// This function returns the PID of the currently running process.
/// It's used by sys_getpid.
///
/// # Returns
///
/// The PID of the current process, or None if no process is running
pub fn get_current_pid() -> Option<u32> {
    SCHEDULER.lock().current()
}

/// Get the parent process PID of the current process
///
/// This function is used by sys_getppid.
///
/// # Returns
///
/// The PPID of the current process, or None if no process is running
pub fn get_current_ppid() -> Option<u32> {
    let table = PROCESS_TABLE.lock();
    let current_pid = SCHEDULER.lock().current()?;
    let process = table.get(current_pid)?;
    Some(process.ppid)
}

/// Add a process to the scheduler
///
/// This function adds a newly created process to the scheduler.
/// The process will be scheduled when its turn comes.
///
/// # Arguments
///
/// * `pid` - PID of the process to add
///
/// # Returns
///
/// * `Ok(())` - Successfully added
/// * `Err(&str)` - Failed to add (process not found, etc.)
pub fn add_process(pid: u32) -> Result<(), &'static str> {
    let table = PROCESS_TABLE.lock();
    let process = table.get(pid).ok_or("Process not found")?;

    // Process should be in Ready state
    if process.state != ProcessState::Ready {
        return Err("Process not in Ready state");
    }

    Ok(())
}

/// Remove a process from the scheduler
///
/// This function is called when a process exits.
///
/// # Arguments
///
/// * `pid` - PID of the process to remove
pub fn remove_process(pid: u32) {
    let mut scheduler = SCHEDULER.lock();

    // If this is the current process, clear current
    if scheduler.current() == Some(pid) {
        scheduler.set_current(0); // Clear current
    }
}

/// Initialize the scheduler
///
/// This function must be called during kernel initialization to set up
/// the scheduler.
pub fn init() {
    let mut scheduler = SCHEDULER.lock();
    scheduler.set_preemption_enabled(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scheduler_new() {
        let scheduler = RoundRobinScheduler::new();
        assert!(scheduler.current().is_none());
        assert_eq!(scheduler.time_slice_ms(), DEFAULT_TIME_SLICE_MS);
        assert!(scheduler.is_preemption_enabled());
    }

    #[test]
    fn test_scheduler_default() {
        let scheduler = RoundRobinScheduler::default();
        assert!(scheduler.current().is_none());
    }
}
