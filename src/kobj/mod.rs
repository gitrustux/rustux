// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel objects reached through handles (Zircon model).
//!
//! - Every object is reference counted (`Arc`); it is destroyed when the last
//!   handle (or kernel reference) goes away, which is also when peers see
//!   `PEER_CLOSED`.
//! - Every object has a [`SignalState`]: a set of signal bits plus the
//!   processes waiting on it. Changing the bits wakes the waiters, who
//!   re-check what they wait for.
//! - Handles live in a per-process [`HandleTable`] and carry [`Rights`].
//!
//! Only process context touches objects (never interrupt handlers), and
//! the kernel is not preempted on its single CPU, so the locks here are
//! never contended. Timers fire lazily: their state is computed when it is
//! looked at, and waiters use the deadline to wake up in time.

pub mod event;
pub mod handle;
pub mod timer;

use crate::sync::SpinMutex;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub use event::{Event, EventPair};
pub use handle::{HandleTable, Rights};
pub use timer::Timer;

/// Signal bits (Zircon numbering)
pub mod signals {
    /// Event / event pair / timer: signalled
    pub const SIGNALED: u32 = 1 << 3;
    /// The other end (event pair, channel) was closed
    pub const PEER_CLOSED: u32 = 1 << 2;
    /// Channel: a message can be read
    pub const READABLE: u32 = 1 << 0;
    /// Channel: a message can be written
    pub const WRITABLE: u32 = 1 << 1;
    /// Process: exited
    pub const TERMINATED: u32 = 1 << 3;
    /// User signals 0..7 (bits 24..31): any object, set by programs
    pub const USER_ALL: u32 = 0xFF00_0000;
}

/// Object types (values match Zircon's ZX_OBJ_TYPE_*)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ObjType {
    Process = 1,
    Thread = 2,
    Vmo = 3,
    Channel = 4,
    Event = 5,
    EventPair = 16,
    Job = 17,
    Vmar = 18,
    Timer = 22,
}

/// Signal bits of an object and the processes waiting for them to change
pub struct SignalState {
    inner: SpinMutex<SignalInner>,
}

struct SignalInner {
    bits: u32,
    waiters: Vec<u32>,
}

impl SignalState {
    pub const fn new(initial: u32) -> Self {
        Self { inner: SpinMutex::new(SignalInner { bits: initial, waiters: Vec::new() }) }
    }

    pub fn get(&self) -> u32 {
        self.inner.lock().bits
    }

    /// Clear then set bits; wakes all waiters if anything changed
    pub fn update(&self, clear: u32, set: u32) {
        let waiters = {
            let mut s = self.inner.lock();
            let new = (s.bits & !clear) | set;
            if new == s.bits {
                return;
            }
            s.bits = new;
            s.waiters.clone()
        };
        for pid in waiters {
            crate::process::run::wake(pid);
        }
    }

    pub fn add_waiter(&self, pid: u32) {
        let mut s = self.inner.lock();
        if !s.waiters.contains(&pid) {
            s.waiters.push(pid);
        }
    }

    pub fn remove_waiter(&self, pid: u32) {
        self.inner.lock().waiters.retain(|&p| p != pid);
    }
}

/// A reference to a kernel object of any type
#[derive(Clone)]
pub enum KObject {
    Event(Arc<Event>),
    EventPair(Arc<EventPair>),
    Timer(Arc<Timer>),
}

impl KObject {
    pub fn obj_type(&self) -> ObjType {
        match self {
            KObject::Event(_) => ObjType::Event,
            KObject::EventPair(_) => ObjType::EventPair,
            KObject::Timer(_) => ObjType::Timer,
        }
    }

    pub fn signal_state(&self) -> &SignalState {
        match self {
            KObject::Event(o) => &o.signals,
            KObject::EventPair(o) => &o.signals,
            KObject::Timer(o) => &o.signals,
        }
    }

    /// Current signals (timers are brought up to date first)
    pub fn signals(&self) -> u32 {
        if let KObject::Timer(t) = self {
            t.refresh();
        }
        self.signal_state().get()
    }

    /// The next time this object's signals may change on their own (a
    /// timer's deadline), so a waiter can wake up for it
    pub fn next_deadline(&self) -> Option<u64> {
        match self {
            KObject::Timer(t) => t.pending_deadline(),
            _ => None,
        }
    }

    /// Bits `object_signal` may change on this object
    pub fn user_settable(&self) -> u32 {
        match self {
            KObject::Event(_) | KObject::EventPair(_) => signals::USER_ALL | signals::SIGNALED,
            _ => signals::USER_ALL,
        }
    }
}

/// Monotonic clock in nanoseconds (TSC based)
pub fn now_ns() -> u64 {
    use crate::arch::amd64::tsc;
    tsc::tsc_to_ns(unsafe { tsc::rdtsc() })
}

/// "Wait forever"
pub const INFINITE: u64 = u64::MAX;
