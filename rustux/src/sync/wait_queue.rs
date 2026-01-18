// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Wait Queue
//!
//! This module provides wait queues for the Rustux kernel.
//! Wait queues are used by synchronization primitives to
//! manage threads that are blocked waiting for an event.
//!
//! # Design
//!
//! - **Priority-ordered**: Threads queued by priority (higher first)
//! - **Fair ordering**: FIFO within same priority level
//! - **Multiple waiters**: Can handle many threads waiting simultaneously
//!
//! # Usage
//!
//! ```rust
//! let wq = WaitQueue::new();
//!
//! // Block current thread on the wait queue
//! wq.block(u64::MAX); // Infinite timeout
//!
//! // Wake one thread
//! wq.wake_one();
//!
//! // Wake all threads
//! wq.wake_all();
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::spinlock::SpinMutex;

/// ============================================================================
/// Types
/// ============================================================================

/// Waiter ID (placeholder until thread module is integrated)
pub type WaiterId = u64;

/// Status code for wait operations
pub type WaitStatus = i32;

/// Success status
pub const WAIT_OK: WaitStatus = 0;

/// Timeout status
pub const WAIT_TIMED_OUT: WaitStatus = -1;

/// ============================================================================
/// Wait Queue Entry
/// ============================================================================

/// Wait queue entry
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WaitQueueEntry {
    /// Waiter ID
    pub waiter_id: WaiterId,

    /// Waiter priority (higher = more important)
    pub priority: u8,

    /// Result code to return when woken
    pub wait_result: WaitStatus,
}

/// ============================================================================
/// Wait Queue
/// ============================================================================

/// Magic number for wait queue validation
const WAIT_QUEUE_MAGIC: u32 = 0x57414954; // "WAIT" in hex

/// Maximum queue depth
const MAX_QUEUE_DEPTH: usize = 256;

/// Wait queue
///
/// Manages entities waiting for a condition to become true.
pub struct WaitQueue {
    /// Queue of waiting entities (simplified Vec-based implementation)
    queue: SpinMutex<WaitQueueInner>,

    /// Magic number for validation
    magic: u32,

    /// Number of entities currently waiting
    count: AtomicUsize,
}

/// Inner queue data
struct WaitQueueInner {
    /// Entries in the queue
    entries: [Option<WaitQueueEntry>; MAX_QUEUE_DEPTH],

    /// Head index
    head: usize,

    /// Tail index
    tail: usize,

    /// Number of entries
    size: usize,
}

impl WaitQueueInner {
    /// Create a new empty inner queue
    const fn new() -> Self {
        const NONE: Option<WaitQueueEntry> = None;
        Self {
            entries: [NONE; MAX_QUEUE_DEPTH],
            head: 0,
            tail: 0,
            size: 0,
        }
    }

    /// Push an entry (sorted by priority)
    fn push_sorted(&mut self, entry: WaitQueueEntry) {
        if self.size >= MAX_QUEUE_DEPTH {
            return; // Queue full
        }

        // Find insertion point (higher priority first)
        let mut insert_pos = self.tail;
        let mut current = self.tail;

        for _ in 0..self.size {
            if let Some(existing) = self.entries[current] {
                if entry.priority > existing.priority {
                    insert_pos = current;
                    break;
                }
            }
            current = (current + 1) % MAX_QUEUE_DEPTH;
        }

        // Shift entries to make room
        let mut pos = self.tail;
        while pos != insert_pos {
            let prev = (pos + MAX_QUEUE_DEPTH - 1) % MAX_QUEUE_DEPTH;
            self.entries[pos] = self.entries[prev];
            pos = prev;
        }

        // Insert new entry
        self.entries[insert_pos] = Some(entry);
        self.tail = (self.tail + 1) % MAX_QUEUE_DEPTH;
        self.size += 1;
    }

    /// Pop an entry from the front (highest priority)
    fn pop_front(&mut self) -> Option<WaitQueueEntry> {
        if self.size == 0 {
            return None;
        }

        let entry = self.entries[self.head];
        self.entries[self.head] = None;
        self.head = (self.head + 1) % MAX_QUEUE_DEPTH;
        self.size -= 1;

        entry
    }

    /// Peek at the front entry
    fn peek_front(&self) -> Option<&WaitQueueEntry> {
        if self.size == 0 {
            return None;
        }
        self.entries[self.head].as_ref()
    }

    /// Check if empty
    fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Get the number of entries
    fn len(&self) -> usize {
        self.size
    }
}

impl WaitQueue {
    /// Create a new wait queue
    pub const fn new() -> Self {
        Self {
            queue: SpinMutex::new(WaitQueueInner::new()),
            magic: WAIT_QUEUE_MAGIC,
            count: AtomicUsize::new(0),
        }
    }

    /// Validate the wait queue magic number
    fn validate(&self) {
        if self.magic != WAIT_QUEUE_MAGIC {
            panic!("wait_queue: invalid magic number");
        }
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.validate();
        self.queue.lock().is_empty()
    }

    /// Get the number of waiters
    pub fn len(&self) -> usize {
        self.validate();
        self.queue.lock().len()
    }

    /// Block the current waiter on this wait queue
    ///
    /// # Arguments
    ///
    /// * `waiter_id` - ID of the waiter
    /// * `priority` - Priority of the waiter (higher = more important)
    /// * `deadline` - Deadline in nanoseconds (u64::MAX = infinite)
    ///
    /// # Returns
    ///
    /// - `Ok(())` if woken successfully
    /// - `Err(WAIT_TIMED_OUT)` if deadline reached
    pub fn block(&self, waiter_id: WaiterId, priority: u8, _deadline: u64) -> WaitStatus {
        self.validate();

        // Add to queue
        {
            let mut queue = self.queue.lock();
            queue.push_sorted(WaitQueueEntry {
                waiter_id,
                priority,
                wait_result: WAIT_OK,
            });
        }

        self.count.fetch_add(1, Ordering::Release);

        // TODO: Integrate with scheduler for proper blocking
        // For now, return immediately (stub)
        WAIT_OK
    }

    /// Wake one waiter (highest priority first)
    ///
    /// # Returns
    ///
    /// - Some(waiter_id) if a waiter was woken
    /// - None if queue was empty
    pub fn wake_one(&self) -> Option<WaiterId> {
        self.validate();

        let entry = {
            let mut queue = self.queue.lock();
            queue.pop_front()
        };

        if let Some(entry) = entry {
            self.count.fetch_sub(1, Ordering::Release);

            // TODO: Integrate with scheduler to actually wake the thread
            Some(entry.waiter_id)
        } else {
            None
        }
    }

    /// Wake all waiters
    ///
    /// # Returns
    ///
    /// Number of waiters woken
    pub fn wake_all(&self) -> usize {
        self.validate();

        let mut count = 0;

        {
            let mut queue = self.queue.lock();
            while let Some(_) = queue.pop_front() {
                count += 1;
            }
        }

        self.count.fetch_sub(count, Ordering::Release);

        // TODO: Integrate with scheduler to actually wake the threads
        count
    }

    /// Get the number of waiters
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_queue_empty() {
        let wq = WaitQueue::new();
        assert!(wq.is_empty());
        assert_eq!(wq.len(), 0);
    }

    #[test]
    fn test_wait_queue_wake_one_empty() {
        let wq = WaitQueue::new();
        assert!(wq.wake_one().is_none());
    }

    #[test]
    fn test_wait_queue_wake_all_empty() {
        let wq = WaitQueue::new();
        assert_eq!(wq.wake_all(), 0);
    }

    #[test]
    fn test_wait_queue_basic() {
        let wq = WaitQueue::new();

        // Add some waiters
        wq.block(1, 10, u64::MAX);
        wq.block(2, 20, u64::MAX);
        wq.block(3, 15, u64::MAX);

        assert_eq!(wq.len(), 3);
        assert!(!wq.is_empty());

        // Wake one (should be highest priority = 20)
        let woken = wq.wake_one();
        assert_eq!(woken, Some(2));
        assert_eq!(wq.len(), 2);
    }

    #[test]
    fn test_wait_queue_priority_order() {
        let wq = WaitQueue::new();

        // Add waiters with different priorities
        wq.block(1, 10, u64::MAX);
        wq.block(2, 30, u64::MAX);
        wq.block(3, 20, u64::MAX);

        // Should wake in priority order: 30, 20, 10
        assert_eq!(wq.wake_one(), Some(2)); // priority 30
        assert_eq!(wq.wake_one(), Some(3)); // priority 20
        assert_eq!(wq.wake_one(), Some(1)); // priority 10
        assert_eq!(wq.wake_one(), None);    // empty
    }

    #[test]
    fn test_wait_queue_wake_all() {
        let wq = WaitQueue::new();

        wq.block(1, 10, u64::MAX);
        wq.block(2, 20, u64::MAX);
        wq.block(3, 15, u64::MAX);

        assert_eq!(wq.wake_all(), 3);
        assert!(wq.is_empty());
    }
}
