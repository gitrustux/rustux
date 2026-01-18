// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Thread representation and management
//!
//! Defines the Thread struct and related types.

use super::state::{ThreadState, ThreadPriority};

/// Thread ID type
pub type ThreadId = u64;

/// Function entry point type
pub type EntryPoint = extern "C" fn(usize) -> !;

/// Thread stack configuration
#[derive(Debug, Clone, Copy)]
pub struct StackConfig {
    /// Base address of the stack
    pub base: usize,
    /// Size of the stack in bytes
    pub size: usize,
    /// Guard page size (0 = no guard page)
    pub guard_size: usize,
}

impl StackConfig {
    /// Create a new stack configuration
    pub fn new(base: usize, size: usize) -> Self {
        Self {
            base,
            size,
            guard_size: 4096,  // 4KB guard page by default
        }
    }

    /// Get the top of the stack (stack grows down)
    pub fn top(&self) -> usize {
        self.base + self.size
    }
}

/// Saved CPU registers
///
/// This represents the saved state of a thread's CPU registers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SavedRegisters {
    /// General purpose registers (x86_64 ABI)
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    /// Return address
    pub rip: u64,
    /// Stack pointer
    pub rsp: u64,
    /// Base pointer (for debugging)
    pub rbp_orig: u64,
}

impl Default for SavedRegisters {
    fn default() -> Self {
        Self {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rsp: 0,
            rbp_orig: 0,
        }
    }
}

/// Thread statistics
#[derive(Debug, Clone, Copy)]
pub struct ThreadStats {
    /// Total CPU time consumed (in cycles)
    pub cpu_time: u64,
    /// Number of times this thread has been scheduled
    pub schedule_count: u64,
    /// Number of voluntary context switches
    pub voluntary_switches: u64,
    /// Number of involuntary context switches
    pub involuntary_switches: u64,
}

impl Default for ThreadStats {
    fn default() -> Self {
        Self {
            cpu_time: 0,
            schedule_count: 0,
            voluntary_switches: 0,
            involuntary_switches: 0,
        }
    }
}

/// Thread structure
///
/// Represents a thread of execution in the kernel.
#[derive(Debug)]
pub struct Thread {
    /// Unique thread ID
    pub id: ThreadId,
    /// Thread entry point
    pub entry_point: EntryPoint,
    /// Entry argument
    pub entry_arg: usize,
    /// Thread state
    pub state: ThreadState,
    /// Thread priority
    pub priority: ThreadPriority,
    /// Saved registers
    pub registers: SavedRegisters,
    /// Stack configuration
    pub stack: StackConfig,
    /// Thread statistics
    pub stats: ThreadStats,
    /// Time slice remaining (in cycles)
    pub time_slice_remaining: u64,
}

impl Thread {
    /// Create a new thread
    pub fn new(id: ThreadId, entry_point: EntryPoint, entry_arg: usize, stack: StackConfig) -> Self {
        let mut thread = Self {
            id,
            entry_point,
            entry_arg,
            state: ThreadState::Ready,
            priority: ThreadPriority::default(),
            registers: SavedRegisters::default(),
            stack,
            stats: ThreadStats::default(),
            time_slice_remaining: 0,
        };

        // Initialize the stack with the entry point
        thread.init_stack();

        thread
    }

    /// Initialize the stack for a new thread
    fn init_stack(&mut self) {
        // Set up the initial stack frame
        // Stack layout (x86_64 System V ABI):
        // ...
        // [arg]        <- rdi (first argument)
        // [ret addr]   <- return address (won't return)
        // [rbp]
        // ...

        unsafe {
            let mut rsp = self.stack.top();

            // Align stack to 16 bytes
            rsp &= !0xF;

            // Push entry argument (will be in rdi when we switch to the thread)
            rsp -= 8;
            *(rsp as *mut usize) = self.entry_arg;

            // Push a fake return address (won't be used since entry_point doesn't return)
            rsp -= 8;
            *(rsp as *mut usize) = 0;

            // Save the stack pointer
            self.registers.rsp = rsp as u64;

            // Set the instruction pointer to the entry point
            self.registers.rip = self.entry_point as u64;
        }
    }

    /// Set the thread priority
    pub fn set_priority(&mut self, priority: ThreadPriority) {
        self.priority = priority;
    }

    /// Get the thread state
    pub fn state(&self) -> ThreadState {
        self.state
    }

    /// Set the thread state
    pub fn set_state(&mut self, state: ThreadState) {
        self.state = state;
    }

    /// Check if the thread is runnable
    pub fn is_runnable(&self) -> bool {
        matches!(self.state, ThreadState::Ready | ThreadState::Running)
    }
}

/// Create a new thread ID
pub fn new_thread_id() -> ThreadId {
    use core::sync::atomic::{AtomicU64, Ordering};

    static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);

    NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed)
}

/// A simple idle thread entry point
///
/// This is used when no other threads are runnable.
pub extern "C" fn idle_thread_entry(_arg: usize) -> ! {
    loop {
        // In a real kernel, this would halt the CPU or enable power saving
        core::hint::spin_loop();
    }
}
