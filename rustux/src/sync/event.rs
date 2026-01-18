// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel Event
//!
//! This module provides event synchronization primitives for the Rustux kernel.
//! Events allow threads to wait for signals and be woken up when the event
//! is signaled.
//!
//! # Design
//!
//! - **Manual reset**: Event remains signaled until explicitly unsignaled
//! - **Auto reset**: Event automatically resets after waking one waiter
//!
//! # Usage
//!
//! ```rust
//! let event = Event::new(false, EventFlags::empty());
//!
//! // Wait for the event (blocks until signaled)
//! event.wait();
//!
//! // Signal the event (wakes waiters)
//! event.signal();
//!
//! // Clear the signal
//! event.unsignal();
//! ```

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::sync::spinlock::SpinMutex;

/// ============================================================================
/// Event Flags
/// ============================================================================

/// Event flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventFlags {
    /// Auto-unsignal after waking one thread
    pub auto_unsignal: bool,

    _reserved: u32,
}

impl EventFlags {
    /// No flags
    pub const fn empty() -> Self {
        Self {
            auto_unsignal: false,
            _reserved: 0,
        }
    }

    /// Auto unsignal flag
    pub const fn auto_unsignal() -> Self {
        Self {
            auto_unsignal: true,
            _reserved: 0,
        }
    }

    /// Convert to raw value
    pub const fn into_raw(self) -> u32 {
        let mut bits = 0u32;
        if self.auto_unsignal {
            bits |= 0x01;
        }
        bits
    }

    /// Convert from raw value
    pub const fn from_raw(bits: u32) -> Self {
        Self {
            auto_unsignal: (bits & 0x01) != 0,
            _reserved: 0,
        }
    }
}

/// ============================================================================
/// Event
/// ============================================================================

/// Magic number for event validation
const EVENT_MAGIC: u32 = 0x45564E54; // "EVNT" in hex

/// Event synchronization primitive
///
/// Threads can wait on events and be woken when the event is signaled.
pub struct Event {
    /// Whether the event is currently signaled
    signaled: AtomicBool,

    /// Event flags
    flags: AtomicU32,

    /// Magic number for validation
    magic: u32,

    /// Number of waiters (for validation on destroy)
    waiter_count: SpinMutex<usize>,
}

impl Event {
    /// Create a new event
    ///
    /// # Arguments
    ///
    /// * `initial` - Initial signaled state
    /// * `flags` - Event flags
    pub const fn new(initial: bool, flags: EventFlags) -> Self {
        Self {
            signaled: AtomicBool::new(initial),
            flags: AtomicU32::new(flags.into_raw()),
            magic: EVENT_MAGIC,
            waiter_count: SpinMutex::new(0),
        }
    }

    /// Initialize an event (for heap-allocated events)
    pub fn init(&mut self, initial: bool, flags: EventFlags) {
        self.signaled.store(initial, Ordering::Release);
        self.flags.store(flags.into_raw(), Ordering::Release);
        self.magic = EVENT_MAGIC;
        *self.waiter_count.lock() = 0;
    }

    /// Destroy an event
    ///
    /// Panics if there are threads still waiting.
    pub fn destroy(&self) {
        self.validate();

        if *self.waiter_count.lock() > 0 {
            panic!("event_destroy: threads still waiting");
        }

        // Clear magic to mark as destroyed
        // Note: This is a const operation issue - in real code we'd use interior mutability
    }

    /// Validate the event magic number
    fn validate(&self) {
        if self.magic != EVENT_MAGIC {
            panic!("event: invalid magic number");
        }
    }

    /// Signal the event
    ///
    /// Wakes up threads waiting on this event.
    pub fn signal(&self) {
        self.validate();

        let flags = EventFlags::from_raw(self.flags.load(Ordering::Relaxed));

        self.signaled.store(true, Ordering::Release);

        // TODO: Wake up waiters
        // For now, this is a no-op since we don't have scheduler integration yet

        if flags.auto_unsignal {
            // Auto-reset: clear the signal after one waiter is woken
            // This will be handled when we integrate with the scheduler
        }
    }

    /// Unsignal the event
    ///
    /// Manually clear the signal.
    pub fn unsignal(&self) {
        self.validate();
        self.signaled.store(false, Ordering::Release);
    }

    /// Check if the event is signaled
    pub fn is_signaled(&self) -> bool {
        self.validate();
        self.signaled.load(Ordering::Relaxed)
    }

    /// Wait for the event to be signaled
    ///
    /// Blocks the current thread until the event is signaled.
    pub fn wait(&self) {
        self.validate();

        // Increment waiter count
        {
            let mut count = self.waiter_count.lock();
            *count += 1;
        }

        // Spin-wait for now (will be replaced with proper blocking when scheduler is integrated)
        while !self.signaled.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        // Decrement waiter count
        {
            let mut count = self.waiter_count.lock();
            *count -= 1;

            // Auto-reset: clear the signal if this is an auto-reset event
            let flags = EventFlags::from_raw(self.flags.load(Ordering::Relaxed));
            if flags.auto_unsignal && *count == 0 {
                self.signaled.store(false, Ordering::Release);
            }
        }
    }

    /// Get the number of waiters
    pub fn waiter_count(&self) -> usize {
        *self.waiter_count.lock()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_basic() {
        let event = Event::new(false, EventFlags::empty());

        assert!(!event.is_signaled());

        event.signal();
        assert!(event.is_signaled());

        event.unsignal();
        assert!(!event.is_signaled());
    }

    #[test]
    fn test_event_initial_signaled() {
        let event = Event::new(true, EventFlags::empty());
        assert!(event.is_signaled());
    }

    #[test]
    fn test_event_flags() {
        let flags = EventFlags::empty();
        assert!(!flags.auto_unsignal);

        let flags = EventFlags::auto_unsignal();
        assert!(flags.auto_unsignal);

        let raw = flags.into_raw();
        let flags2 = EventFlags::from_raw(raw);
        assert_eq!(flags.auto_unsignal, flags2.auto_unsignal);
    }

    #[test]
    fn test_event_wait() {
        let event = Event::new(false, EventFlags::empty());

        // Signal in a different context
        // For now, just test the basic mechanics
        assert!(!event.is_signaled());
        event.signal();
        assert!(event.is_signaled());
    }
}
