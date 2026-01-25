// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Event Objects
//!
//! Events are simple synchronization primitives that can be signaled
//! and waited upon. They support both auto-reset and manual-reset modes.
//!
//! # Design
//!
//! - **Simple signaling**: Binary state (signaled/not signaled)
//! - **Auto-reset**: Automatically clears when a waiter wakes
//! - **Manual-reset**: Remains signaled until explicitly cleared
//! - **Wait queues**: Multiple threads can wait on same event
//!
//! # Usage
//!
//! ```rust
//! let event = Event::new(false, EventFlags::MANUAL_RESET)?;
//! event.signal();
//! event.wait()?;
//! event.unsignal();
//! ```

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::sync::SpinMutex;
use crate::sync::WaitQueue;
use crate::object::handle::{KernelObjectBase, ObjectType};

/// ============================================================================
/// Event ID
/// ============================================================================

/// Event identifier
pub type EventId = u64;

/// Next event ID counter
static mut NEXT_EVENT_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new event ID
fn alloc_event_id() -> EventId {
    unsafe { NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed) }
}

/// ============================================================================
/// Event Flags
/// ============================================================================

/// Event creation flags
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventFlags(pub u32);

impl EventFlags {
    /// No flags
    pub const empty: Self = Self(0);

    /// Manual reset (stays signaled until explicitly cleared)
    pub const MANUAL_RESET: Self = Self(0x01);

    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self.0
    }

    /// Check if manual reset
    pub const fn is_manual_reset(self) -> bool {
        (self.0 & Self::MANUAL_RESET.0) != 0
    }
}

/// ============================================================================
/// Event
/// ============================================================================

/// Event object
///
/// A simple synchronization primitive for signaling between threads.
pub struct Event {
    /// Kernel object base
    pub base: KernelObjectBase,

    /// Event ID
    pub id: EventId,

    /// Current signal state
    pub signaled: AtomicBool,

    /// Event flags
    pub flags: EventFlags,

    /// Wait queue for blocked waiters
    pub waiters: SpinMutex<WaitQueue>,
}

impl Event {
    /// Create a new event
    ///
    /// # Arguments
    ///
    /// * `signaled` - Initial signal state
    /// * `flags` - Event flags
    pub fn new(signaled: bool, flags: EventFlags) -> Self {
        Self {
            base: KernelObjectBase::new(ObjectType::Event),
            id: alloc_event_id(),
            signaled: AtomicBool::new(signaled),
            flags,
            waiters: SpinMutex::new(WaitQueue::new()),
        }
    }

    /// Get event ID
    pub const fn id(&self) -> EventId {
        self.id
    }

    /// Check if event is signaled
    pub fn is_signaled(&self) -> bool {
        self.signaled.load(Ordering::Acquire)
    }

    /// Signal the event
    ///
    /// Wakes up all waiting threads.
    pub fn signal(&self) {
        self.signaled.store(true, Ordering::Release);

        // Wake all waiters (interior mutability through Mutex)
        let waiters = self.waiters.lock();
        waiters.wake_all();
    }

    /// Unsignal the event
    ///
    /// Clears the signal state (for manual-reset events).
    pub fn unsignal(&self) {
        self.signaled.store(false, Ordering::Release);
    }

    /// Wait for the event to be signaled
    ///
    /// Blocks the current thread until the event is signaled.
    /// For auto-reset events, automatically clears the signal.
    pub fn wait(&self) -> Result<(), &'static str> {
        // Check if already signaled
        if self.is_signaled() {
            if !self.flags.is_manual_reset() {
                // Auto-reset: clear the signal
                self.signaled.store(false, Ordering::Release);
            }
            return Ok(());
        }

        // Block until signaled
        // TODO: Integrate with scheduler for proper blocking
        // For now, spin-wait
        while !self.is_signaled() {
            core::hint::spin_loop();
        }

        if !self.flags.is_manual_reset() {
            // Auto-reset: clear the signal
            self.signaled.store(false, Ordering::Release);
        }

        Ok(())
    }

    /// Get the kernel object base
    pub fn base(&self) -> &KernelObjectBase {
        &self.base
    }

    /// Get reference count
    pub fn ref_count(&self) -> usize {
        self.base.ref_count()
    }

    /// Increment reference count
    pub fn ref_inc(&self) {
        self.base.ref_inc();
    }

    /// Decrement reference count
    ///
    /// Returns true if this was the last reference.
    pub fn ref_dec(&self) -> bool {
        self.base.ref_dec()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_flags() {
        let flags = EventFlags::empty();
        assert!(!flags.is_manual_reset());

        let flags = EventFlags::MANUAL_RESET;
        assert!(flags.is_manual_reset());

        let flags = EventFlags::from_raw(0x01);
        assert!(flags.is_manual_reset());
    }

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
    fn test_event_manual_reset() {
        let event = Event::new(false, EventFlags::MANUAL_RESET);

        event.signal();
        assert!(event.is_signaled());

        // Wait should succeed but signal should remain (manual reset)
        event.wait().unwrap();
        assert!(event.is_signaled());

        // Explicitly unsignal
        event.unsignal();
        assert!(!event.is_signaled());
    }

    #[test]
    fn test_event_auto_reset() {
        let event = Event::new(false, EventFlags::empty());

        event.signal();
        assert!(event.is_signaled());

        // Wait should succeed and clear the signal (auto reset)
        event.wait().unwrap();
        assert!(!event.is_signaled());
    }

    #[test]
    fn test_event_initially_signaled() {
        let event = Event::new(true, EventFlags::empty());
        assert!(event.is_signaled());

        // Wait should succeed immediately and clear signal
        event.wait().unwrap();
        assert!(!event.is_signaled());
    }

    #[test]
    fn test_event_ref_count() {
        let event = Event::new(false, EventFlags::empty());

        assert_eq!(event.ref_count(), 1);

        event.ref_inc();
        assert_eq!(event.ref_count(), 2);

        assert!(!event.ref_dec());
        assert_eq!(event.ref_count(), 1);

        assert!(event.ref_dec());
    }
}
