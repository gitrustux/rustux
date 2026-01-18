// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Timer Objects
//!
//! Timer objects provide high-resolution timers for user-space processes.
//! They support one-shot and periodic timers.
//!
//! # Design
//!
//! - **High-resolution**: Nanosecond precision
//! - **One-shot**: Fire once at specified deadline
//! - **Periodic**: Fire repeatedly at specified interval
//! - **Slack**: Allow coalescing for power efficiency
//!
//! # Usage
//!
//! ```rust
//! let timer = Timer::create()?;
//! timer.set(deadline, slack)?;
//! timer.wait()?;
//! ```

use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use crate::sync::SpinMutex;
use crate::object::handle::{KernelObjectBase, ObjectType};
use crate::object::event::Event;

/// ============================================================================
/// Timer ID
/// ============================================================================

/// Timer identifier
pub type TimerId = u64;

/// Next timer ID counter
static mut NEXT_TIMER_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new timer ID
fn alloc_timer_id() -> TimerId {
    unsafe { NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed) }
}

/// ============================================================================
/// Timer State
/// ============================================================================

/// Timer state
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    /// Timer is disarmed
    Disarmed = 0,

    /// Timer is armed (waiting for deadline)
    Armed = 1,

    /// Timer has fired
    Fired = 2,

    /// Timer was canceled
    Canceled = 3,
}

impl TimerState {
    /// Create from raw value
    pub const fn from_raw(raw: u8) -> Self {
        match raw {
            1 => Self::Armed,
            2 => Self::Fired,
            3 => Self::Canceled,
            _ => Self::Disarmed,
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> u8 {
        self as u8
    }
}

/// ============================================================================
/// Slack Policy
/// ============================================================================

/// Timer slack policy
///
/// Determines how much the timer deadline can be adjusted for power efficiency.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlackPolicy {
    /// No slack (precise timing)
    None = 0,

    /// Small slack (default)
    Small = 1,

    /// Medium slack
    Medium = 2,

    /// Large slack (maximum coalescing)
    Large = 3,
}

impl SlackPolicy {
    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Small,
            2 => Self::Medium,
            3 => Self::Large,
            _ => Self::None,
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self as u32
    }

    /// Get slack duration in nanoseconds
    pub const fn duration(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Small => 100_000,       // 100us
            Self::Medium => 1_000_000,    // 1ms
            Self::Large => 10_000_000,    // 10ms
        }
    }
}

/// ============================================================================
/// Timer
/// ============================================================================

/// Timer object
///
/// Provides high-resolution timer functionality.
pub struct Timer {
    /// Kernel object base
    pub base: KernelObjectBase,

    /// Timer ID
    pub id: TimerId,

    /// Timer deadline (in nanoseconds)
    pub deadline: AtomicU64,

    /// Timer slack (in nanoseconds)
    pub slack: AtomicU64,

    /// Timer period (None = one-shot, Some = periodic)
    pub period: SpinMutex<Option<NonZeroU64>>,

    /// Timer state
    pub state: AtomicU8,

    /// Event signaled when timer fires
    pub event: SpinMutex<Event>,

    /// Slack policy
    pub slack_policy: SpinMutex<SlackPolicy>,
}

impl Timer {
    /// Create a new timer
    ///
    /// Initially disarmed.
    pub fn create() -> Result<Self, &'static str> {
        Ok(Self {
            base: KernelObjectBase::new(ObjectType::Timer),
            id: alloc_timer_id(),
            deadline: AtomicU64::new(0),
            slack: AtomicU64::new(0),
            period: SpinMutex::new(None),
            state: AtomicU8::new(TimerState::Disarmed as u8),
            event: SpinMutex::new(Event::new(false, crate::object::event::EventFlags::empty)),
            slack_policy: SpinMutex::new(SlackPolicy::Small),
        })
    }

    /// Get timer ID
    pub const fn id(&self) -> TimerId {
        self.id
    }

    /// Get timer state
    pub fn state(&self) -> TimerState {
        TimerState::from_raw(self.state.load(Ordering::Acquire))
    }

    /// Set the timer
    ///
    /// # Arguments
    ///
    /// * `deadline` - Absolute deadline in nanoseconds
    /// * `slack` - Optional slack duration in nanoseconds
    ///
    /// If the timer is already armed, this cancels the previous deadline.
    pub fn set(&self, deadline: u64, slack: Option<u64>) -> Result<(), &'static str> {
        // Update deadline and slack
        self.deadline.store(deadline, Ordering::Release);
        self.slack.store(slack.unwrap_or(0), Ordering::Release);

        // Update state
        self.state.store(TimerState::Armed as u8, Ordering::Release);

        // Unsignal event
        self.event.lock().unsignal();

        // TODO: Add to global timer queue

        Ok(())
    }

    /// Set a periodic timer
    ///
    /// # Arguments
    ///
    /// * `deadline` - First deadline in nanoseconds
    /// * `period` - Period in nanoseconds
    /// * `slack` - Optional slack duration in nanoseconds
    pub fn set_periodic(&self, deadline: u64, period: u64, slack: Option<u64>) -> Result<(), &'static str> {
        if period == 0 {
            return Err("period cannot be zero");
        }

        // Set period
        *self.period.lock() = Some(NonZeroU64::new(period).unwrap());

        // Set timer
        self.set(deadline, slack)
    }

    /// Cancel the timer
    ///
    /// # Returns
    ///
    /// - Ok(()) if timer was canceled
    /// - Err("timer not armed") if timer was not armed
    pub fn cancel(&self) -> Result<(), &'static str> {
        let state = self.state();

        match state {
            TimerState::Disarmed | TimerState::Canceled => {
                return Err("timer not armed");
            }
            TimerState::Fired => {
                return Err("timer already fired");
            }
            TimerState::Armed => {
                // Cancel timer
                self.state.store(TimerState::Canceled as u8, Ordering::Release);

                // TODO: Remove from global timer queue

                // Unsignal event
                self.event.lock().unsignal();

                Ok(())
            }
        }
    }

    /// Wait for timer to fire
    ///
    /// # Returns
    ///
    /// - Ok(()) if timer fired
    /// - Err("canceled") if timer was canceled
    pub fn wait(&self) -> Result<(), &'static str> {
        // Wait on event
        self.event.lock().wait()?;

        // Check if timer was canceled
        if self.state() == TimerState::Canceled {
            return Err("timer canceled");
        }

        Ok(())
    }

    /// Get current deadline
    pub fn deadline(&self) -> u64 {
        self.deadline.load(Ordering::Acquire)
    }

    /// Get current slack
    pub fn slack(&self) -> u64 {
        self.slack.load(Ordering::Acquire)
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
    fn test_timer_state() {
        assert_eq!(TimerState::from_raw(1), TimerState::Armed);
        assert_eq!(TimerState::from_raw(2), TimerState::Fired);
        assert_eq!(TimerState::from_raw(3), TimerState::Canceled);
        assert_eq!(TimerState::from_raw(99), TimerState::Disarmed);

        assert_eq!(TimerState::Armed.into_raw(), 1);
    }

    #[test]
    fn test_slack_policy() {
        assert_eq!(SlackPolicy::from_raw(0), SlackPolicy::None);
        assert_eq!(SlackPolicy::from_raw(1), SlackPolicy::Small);
        assert_eq!(SlackPolicy::from_raw(2), SlackPolicy::Medium);
        assert_eq!(SlackPolicy::from_raw(3), SlackPolicy::Large);

        assert_eq!(SlackPolicy::Small.duration(), 100_000);
        assert_eq!(SlackPolicy::Medium.duration(), 1_000_000);
        assert_eq!(SlackPolicy::Large.duration(), 10_000_000);
    }

    #[test]
    fn test_timer_create() {
        let timer = Timer::create().unwrap();
        assert_eq!(timer.state(), TimerState::Disarmed);
        assert_eq!(timer.deadline(), 0);
        assert_eq!(timer.slack(), 0);
    }

    #[test]
    fn test_timer_set() {
        let timer = Timer::create().unwrap();

        timer.set(1_000_000, Some(100)).unwrap();
        assert_eq!(timer.state(), TimerState::Armed);
        assert_eq!(timer.deadline(), 1_000_000);
        assert_eq!(timer.slack(), 100);
    }

    #[test]
    fn test_timer_cancel() {
        let timer = Timer::create().unwrap();

        // Cannot cancel when not armed
        assert!(timer.cancel().is_err());

        timer.set(1_000_000, None).unwrap();
        assert_eq!(timer.state(), TimerState::Armed);

        timer.cancel().unwrap();
        assert_eq!(timer.state(), TimerState::Canceled);
    }

    #[test]
    fn test_timer_periodic() {
        let timer = Timer::create().unwrap();

        timer.set_periodic(1_000_000, 100_000, None).unwrap();
        assert_eq!(timer.state(), TimerState::Armed);

        let period = *timer.period.lock();
        assert_eq!(period.map(|p| p.get()), Some(100_000));
    }

    #[test]
    fn test_timer_periodic_zero() {
        let timer = Timer::create().unwrap();

        // Period cannot be zero
        assert!(timer.set_periodic(1_000_000, 0, None).is_err());
    }
}
