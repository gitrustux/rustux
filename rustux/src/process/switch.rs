// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Process Context Switch
//!
//! This module provides the Rust interface to the low-level context
//! switch assembly code. It handles switching between processes by
//! saving the current process's state and restoring the next process's
//! state.

use crate::process::table::{Process, SavedState};

/// ============================================================================
/// Assembly Context Switch Function
/// ============================================================================

extern "C" {
    /// Low-level context switch assembly function
    ///
    /// # Arguments
    ///
    /// * `prev` - Pointer to SavedState to save current state
    /// * `next` - Pointer to SavedState to restore next state
    /// * `next_cr3` - CR3 value (page table) for the next process
    ///
    /// # Safety
    ///
    /// This function must only be called when:
    /// - The `prev` and `next` pointers point to valid SavedState
    /// - The `next_cr3` points to valid page tables
    /// - The stack is valid for the current process
    ///
    /// This function never returns in the normal sense - it "returns"
    /// to the RIP saved in the next process's SavedState.
    fn context_switch(prev: *mut SavedState, next: *const SavedState, next_cr3: u64);
}

/// ============================================================================
/// High-Level Context Switch API
/// ============================================================================

/// Raw context switch using pointers
///
/// This is a lower-level version that takes raw pointers instead of
/// references. It's used internally by the scheduler when it needs
/// to work around Rust's borrowing rules.
///
/// # Arguments
///
/// * `prev` - Pointer to SavedState to save current state
/// * `next` - Pointer to SavedState to restore next state
/// * `next_cr3` - CR3 value for the next process
///
/// # Safety
///
/// This function is extremely unsafe and should only be called by
/// the scheduler with proper locking.
pub unsafe fn context_switch_raw(
    prev: *mut SavedState,
    next: *const SavedState,
    next_cr3: u64,
) {
    context_switch(prev, next, next_cr3);
}

/// Switch from one process to another
///
/// This function saves the current process's CPU state and restores
/// the next process's CPU state, then jumps to the next process's
/// instruction pointer.
///
/// # Arguments
///
/// * `current` - Mutable reference to the current process
/// * `next` - Reference to the next process to run
///
/// # Safety
///
/// This function is unsafe because:
/// - It performs a low-level context switch
/// - The caller must ensure both processes are valid
/// - The caller must ensure the next process's page tables are valid
/// - This function changes the current execution context
///
/// After calling this function, code execution continues in the
/// next process at its saved RIP. The current process will later
/// be resumed when another context switch back to it occurs.
pub unsafe fn switch_to(current: &mut Process, next: &Process) {
    // Update process states
    current.state = crate::process::table::ProcessState::Ready;

    // Perform the context switch
    // The assembly function will save current's state to current.saved_state
    // and restore next's state from next.saved_state
    context_switch(
        &mut current.saved_state as *mut SavedState,
        &next.saved_state as *const SavedState,
        next.page_table,
    );

    // After returning here, we are now executing as the `next` process
    // (or we've been switched back to this process later)
}

/// Switch to a process by PID
///
/// This is a convenience wrapper that looks up the process by PID
/// and then calls switch_to().
///
/// # Arguments
///
/// * `next_pid` - PID of the process to switch to
///
/// # Returns
///
/// * `Ok(())` - Successfully switched to the next process
/// * `Err(&str)` - Failed to switch (process not found, etc.)
///
/// # Safety
///
/// This function is unsafe because it performs a context switch.
/// The caller must ensure proper locking and state management.
pub unsafe fn switch_to_pid(next_pid: u32) -> Result<(), &'static str> {
    use crate::process::table::PROCESS_TABLE;

    let mut table = PROCESS_TABLE.lock();

    // Get current process
    let current_pid = table
        .current_pid()
        .ok_or("No current process")?;

    // Can't switch to ourselves
    if current_pid == next_pid {
        return Ok(());
    }

    // Extract the data we need first to avoid borrowing issues
    let next_cr3 = table.get(next_pid)
        .map(|p| p.page_table)
        .ok_or("Next process not found")?;

    let next_state = table.get(next_pid)
        .map(|p| p.state)
        .ok_or("Next process not found")?;

    // Check that next process is runnable
    if !next_state.is_runnable() {
        return Err("Next process is not runnable");
    }

    // Get the saved state pointers
    let next_saved_ptr = table.get(next_pid)
        .map(|p| &p.saved_state as *const SavedState)
        .ok_or("Next process not found")?;

    let current_saved_ptr = table.get_mut(current_pid)
        .map(|p| &mut p.saved_state as *mut SavedState)
        .ok_or("Current process not found")?;

    // Update current process state
    if let Some(process) = table.get_mut(current_pid) {
        process.state = crate::process::table::ProcessState::Ready;
    }

    // Update the table's current pointer
    table.set_current(next_pid);

    // Mark next as running
    if let Some(process) = table.get_mut(next_pid) {
        process.state = crate::process::table::ProcessState::Running;
    }

    // Release the table lock before context switch
    // (we can't hold locks across context switches)
    drop(table);

    // Perform the context switch
    context_switch(current_saved_ptr, next_saved_ptr, next_cr3);

    Ok(())
}

/// Initialize the SavedState for a new process
///
/// This function creates a SavedState for a new userspace process
/// that will start execution at the given entry point.
///
/// # Arguments
///
/// * `entry` - Entry point address (RIP)
/// * `user_stack_top` - Top of user stack (RSP)
/// * `cr3` - Page table physical address
///
/// # Returns
///
/// A new SavedState initialized for userspace execution
pub fn init_userspace_state(entry: u64, user_stack_top: u64, cr3: u64) -> SavedState {
    SavedState::for_userspace(entry, user_stack_top, cr3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_saved_state_creation() {
        let state = init_userspace_state(0x1000, 0x7000_0000_0000, 0x5000);
        assert_eq!(state.rip, 0x1000);
        assert_eq!(state.rsp, 0x7000_0000_0000);
        assert_eq!(state.cr3, 0x5000);
    }
}
