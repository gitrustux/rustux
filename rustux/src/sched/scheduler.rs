// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Scheduler implementation
//!
//! Provides a simple round-robin scheduler with priority support.

use super::thread::{Thread, ThreadId, new_thread_id};
use super::state::{RunQueue, ThreadState};

/// Default time slice for threads (in CPU cycles)
const DEFAULT_TIME_SLICE: u64 = 10_000_000;  // ~10ms at 1GHz

/// Maximum number of threads in the system
const MAX_THREADS: usize = 1024;

/// Scheduler
///
/// Manages thread scheduling and context switching.
pub struct Scheduler {
    /// Run queue
    run_queue: RunQueue,
    /// Currently running thread (per CPU)
    current_thread: Option<ThreadId>,
    /// All threads in the system
    threads: [Option<Thread>; MAX_THREADS],
    /// Number of threads in the system
    thread_count: usize,
    /// Scheduling policy
    policy: SchedulingPolicy,
    /// Preemption enabled
    preemption_enabled: bool,
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new() -> Self {
        Self {
            run_queue: RunQueue::new(),
            current_thread: None,
            threads: [const { None }; MAX_THREADS],
            thread_count: 0,
            policy: SchedulingPolicy::RoundRobin,
            preemption_enabled: true,
        }
    }

    /// Add a thread to the scheduler
    pub fn add_thread(&mut self, thread: Thread) -> Result<(), &'static str> {
        if self.thread_count >= MAX_THREADS {
            return Err("Maximum number of threads reached");
        }

        let thread_id = thread.id;

        // Find a free slot
        for slot in &mut self.threads {
            if slot.is_none() {
                *slot = Some(thread);
                self.thread_count += 1;

                // Add to run queue
                self.enqueue_thread(thread_id);
                return Ok(());
            }
        }

        Err("No free thread slots available")
    }

    /// Remove a thread from the scheduler
    pub fn remove_thread(&mut self, thread_id: ThreadId) -> Option<Thread> {
        // Remove from run queue
        self.run_queue.remove(thread_id);

        // Remove from threads array
        for slot in &mut self.threads {
            if let Some(thread) = slot {
                if thread.id == thread_id {
                    self.thread_count -= 1;
                    return slot.take();
                }
            }
        }

        None
    }

    /// Enqueue a thread onto the run queue
    fn enqueue_thread(&mut self, thread_id: ThreadId) {
        // First, get the thread data without holding the borrow
        let (is_runnable, priority) = if let Some(thread) = self.get_thread(thread_id) {
            (thread.is_runnable(), thread.priority)
        } else {
            return;
        };

        if is_runnable {
            use super::state::RunQueueEntry;
            self.run_queue.enqueue(RunQueueEntry {
                thread_id,
                priority,
                time_slice: DEFAULT_TIME_SLICE,
            });
        }
    }

    /// Get a mutable reference to a thread
    fn get_thread_mut(&mut self, thread_id: ThreadId) -> Option<&mut Thread> {
        self.threads.iter_mut().find_map(|slot| {
            slot.as_mut().filter(|t| t.id == thread_id)
        })
    }

    /// Get an immutable reference to a thread
    fn get_thread(&self, thread_id: ThreadId) -> Option<&Thread> {
        self.threads.iter().find_map(|slot| {
            slot.as_ref().filter(|t| t.id == thread_id)
        })
    }

    /// Schedule the next thread to run
    ///
    /// This implements the core scheduling algorithm.
    /// For round-robin, it picks the highest-priority ready thread.
    pub fn schedule(&mut self) -> Option<ThreadId> {
        // Get the next thread from the run queue
        if let Some(entry) = self.run_queue.dequeue() {
            // Mark the current thread as ready (if there is one)
            if let Some(current_id) = self.current_thread {
                // First, check if we need to re-queue the current thread
                let should_requeue = if let Some(current) = self.get_thread(current_id) {
                    current.state == ThreadState::Running
                } else {
                    false
                };

                if should_requeue {
                    // Get the priority for re-queuing
                    let priority = if let Some(current) = self.get_thread(current_id) {
                        current.priority
                    } else {
                        super::state::ThreadPriority::Normal
                    };

                    // Update the current thread state
                    if let Some(current) = self.get_thread_mut(current_id) {
                        current.set_state(ThreadState::Ready);
                    }

                    // Re-queue the current thread
                    use super::state::RunQueueEntry;
                    self.run_queue.enqueue(RunQueueEntry {
                        thread_id: current_id,
                        priority,
                        time_slice: DEFAULT_TIME_SLICE,
                    });
                }
            }

            // Set the new thread as running
            if let Some(thread) = self.get_thread_mut(entry.thread_id) {
                thread.set_state(ThreadState::Running);
                thread.stats.schedule_count += 1;
                thread.time_slice_remaining = entry.time_slice;
            }

            self.current_thread = Some(entry.thread_id);
            Some(entry.thread_id)
        } else {
            // No threads to run, stay with current or go idle
            if self.current_thread.is_some() {
                self.current_thread
            } else {
                // Create idle thread
                let idle_id = new_thread_id();
                let idle_thread = Thread::new(
                    idle_id,
                    super::thread::idle_thread_entry,
                    0,
                    super::thread::StackConfig::new(0, 0),  // No stack for idle
                );
                self.add_thread(idle_thread).ok()?;
                self.current_thread = Some(idle_id);
                Some(idle_id)
            }
        }
    }

    /// Get the currently running thread
    pub fn current_thread(&self) -> Option<ThreadId> {
        self.current_thread
    }

    /// Get the current thread as a mutable reference
    pub fn current_thread_mut(&mut self) -> Option<&mut Thread> {
        self.current_thread.and_then(|id| self.get_thread_mut(id))
    }

    /// Yield the CPU to another thread
    pub fn yield_cpu(&mut self) {
        if let Some(current_id) = self.current_thread {
            if let Some(current) = self.get_thread_mut(current_id) {
                if current.state == ThreadState::Running {
                    current.set_state(ThreadState::Ready);
                    current.stats.voluntary_switches += 1;
                }
            }
        }
        self.schedule();
    }

    /// Block the current thread
    pub fn block_current_thread(&mut self, new_state: ThreadState) {
        if let Some(current_id) = self.current_thread {
            if let Some(current) = self.get_thread_mut(current_id) {
                current.set_state(new_state);
                current.stats.involuntary_switches += 1;
            }
        }
        self.schedule();
    }

    /// Wake up a blocked thread
    pub fn wake_thread(&mut self, thread_id: ThreadId) -> Result<(), &'static str> {
        // First, check if the thread exists and is blocked
        let is_blocked = if let Some(thread) = self.get_thread(thread_id) {
            matches!(thread.state, ThreadState::Blocked | ThreadState::BlockedOnMutex | ThreadState::BlockedOnCondvar)
        } else {
            return Err("Thread not found");
        };

        if !is_blocked {
            return Err("Thread is not blocked");
        }

        // Now update the thread state
        if let Some(thread) = self.get_thread_mut(thread_id) {
            thread.set_state(ThreadState::Ready);
        }

        // Enqueue the thread
        self.enqueue_thread(thread_id);
        Ok(())
    }

    /// Get the number of threads in the system
    pub fn thread_count(&self) -> usize {
        self.thread_count
    }

    /// Get the number of threads in the run queue
    pub fn run_queue_len(&self) -> usize {
        self.run_queue.len()
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Scheduling policies
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingPolicy {
    /// Simple round-robin scheduling
    RoundRobin,
    /// Priority-based scheduling
    Priority,
    /// Completely fair scheduler
    Cfs,
}

/// Per-CPU scheduler state
///
/// In an SMP system, each CPU has its own scheduler.
pub struct PerCpuScheduler {
    /// CPU ID
    pub cpu_id: u32,
    /// Local scheduler
    pub scheduler: Scheduler,
    /// Load balancing enabled
    pub load_balancing: bool,
}

impl PerCpuScheduler {
    /// Create a new per-CPU scheduler
    pub fn new(cpu_id: u32) -> Self {
        Self {
            cpu_id,
            scheduler: Scheduler::new(),
            load_balancing: true,
        }
    }
}
