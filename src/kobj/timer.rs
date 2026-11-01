// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Timers: become `SIGNALED` at a deadline (monotonic ns).
//!
//! Lazy: nothing runs at the deadline. The state is brought up to date
//! whenever someone looks ([`Timer::refresh`]), and waiters block with the
//! timer's deadline as their wake-up time.

use super::{now_ns, signals, SignalState};
use crate::sync::SpinMutex;
use alloc::sync::Arc;

pub struct Timer {
    pub signals: SignalState,
    /// Armed deadline, if any
    deadline: SpinMutex<Option<u64>>,
}

impl Timer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0), deadline: SpinMutex::new(None) })
    }

    /// Arm (or re-arm) for `deadline`; clears `SIGNALED`
    pub fn set(&self, deadline: u64) {
        *self.deadline.lock() = Some(deadline);
        self.signals.update(signals::SIGNALED, 0);
        self.refresh();
    }

    /// Disarm; clears `SIGNALED`
    pub fn cancel(&self) {
        *self.deadline.lock() = None;
        self.signals.update(signals::SIGNALED, 0);
    }

    /// Fire if the deadline has passed
    pub fn refresh(&self) {
        let fire = {
            let mut d = self.deadline.lock();
            match *d {
                Some(t) if now_ns() >= t => {
                    *d = None;
                    true
                }
                _ => false,
            }
        };
        if fire {
            self.signals.update(0, signals::SIGNALED);
        }
    }

    pub fn pending_deadline(&self) -> Option<u64> {
        *self.deadline.lock()
    }
}
