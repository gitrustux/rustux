// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Thread state and run queue
//!
//! Defines thread states and run queue data structures.

/// Thread states
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Thread is ready to run
    Ready,
    /// Thread is currently running
    Running,
    /// Thread is blocked (waiting for I/O, event, etc.)
    Blocked,
    /// Thread has terminated
    Terminated,
    /// Thread is sleeping
    Sleeping,
    /// Thread is waiting for a mutex
    BlockedOnMutex,
    /// Thread is waiting for a condition variable
    BlockedOnCondvar,
}

/// Thread priority levels
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadPriority {
    /// Idle priority (lowest)
    Idle = 0,
    /// Low priority
    Low = 1,
    /// Normal priority (default)
    Normal = 2,
    /// High priority
    High = 3,
    /// Real-time priority (highest)
    Realtime = 4,
}

impl Default for ThreadPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Run queue entry
#[derive(Debug, Clone, Copy)]
pub struct RunQueueEntry {
    /// Thread ID
    pub thread_id: u64,
    /// Thread priority
    pub priority: ThreadPriority,
    /// Time slice remaining
    pub time_slice: u64,
}

/// Run queue
///
/// Simple round-robin run queue organized by priority.
/// Uses fixed-size arrays with head/tail pointers.
#[derive(Debug)]
pub struct RunQueue {
    /// Queue entries for each priority level
    queues: [[RunQueueEntry; 64]; 5],
    /// Head pointer for each priority level
    heads: [usize; 5],
    /// Tail pointer for each priority level
    tails: [usize; 5],
    /// Count of entries in each priority level
    counts: [usize; 5],
    /// Total number of threads in the run queue
    total_count: usize,
}

/// Empty run queue entry
const EMPTY_ENTRY: RunQueueEntry = RunQueueEntry {
    thread_id: 0,
    priority: ThreadPriority::Idle,
    time_slice: 0,
};

impl RunQueue {
    /// Maximum number of threads per priority level
    const MAX_PER_PRIORITY: usize = 64;

    /// Create a new empty run queue
    pub fn new() -> Self {
        Self {
            queues: [[EMPTY_ENTRY; Self::MAX_PER_PRIORITY]; 5],
            heads: [0; 5],
            tails: [0; 5],
            counts: [0; 5],
            total_count: 0,
        }
    }

    /// Add a thread to the run queue
    pub fn enqueue(&mut self, entry: RunQueueEntry) {
        let priority_idx = entry.priority as usize;
        if priority_idx >= self.queues.len() {
            return;
        }

        let tail = self.tails[priority_idx];
        if self.counts[priority_idx] < Self::MAX_PER_PRIORITY {
            self.queues[priority_idx][tail % Self::MAX_PER_PRIORITY] = entry;
            self.tails[priority_idx] = tail + 1;
            self.counts[priority_idx] += 1;
            self.total_count += 1;
        }
    }

    /// Remove the next thread to run (highest priority first)
    pub fn dequeue(&mut self) -> Option<RunQueueEntry> {
        // Find the highest priority non-empty queue
        for i in (0..5).rev() {
            if self.counts[i] > 0 {
                let head = self.heads[i];
                let entry = self.queues[i][head % Self::MAX_PER_PRIORITY];
                self.heads[i] = head + 1;
                self.counts[i] -= 1;
                self.total_count -= 1;
                return Some(entry);
            }
        }
        None
    }

    /// Check if the run queue is empty
    pub fn is_empty(&self) -> bool {
        self.total_count == 0
    }

    /// Get the number of threads in the run queue
    pub fn len(&self) -> usize {
        self.total_count
    }

    /// Remove a specific thread from the run queue
    pub fn remove(&mut self, thread_id: u64) -> bool {
        for priority_idx in 0..5 {
            let count = self.counts[priority_idx];
            if count == 0 {
                continue;
            }

            let head = self.heads[priority_idx];
            for i in 0..count {
                let idx = (head + i) % Self::MAX_PER_PRIORITY;
                if self.queues[priority_idx][idx].thread_id == thread_id {
                    // Found the thread - remove it by shifting entries
                    for j in i..count - 1 {
                        let src_idx = (head + j + 1) % Self::MAX_PER_PRIORITY;
                        let dst_idx = (head + j) % Self::MAX_PER_PRIORITY;
                        self.queues[priority_idx][dst_idx] = self.queues[priority_idx][src_idx];
                    }
                    self.tails[priority_idx] -= 1;
                    self.counts[priority_idx] -= 1;
                    self.total_count -= 1;
                    return true;
                }
            }
        }
        false
    }
}

impl Default for RunQueue {
    fn default() -> Self {
        Self::new()
    }
}
