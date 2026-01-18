// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel Synchronization Primitives
//!
//! This module provides core synchronization primitives for the Rustux kernel.
//! These are designed for kernel-internal use and provide proper locking,
//! waiting, and signaling mechanisms.
//!
//! # Primitives
//!
//! - **SpinMutex**: Spin-based mutual exclusion lock for short critical sections
//! - **Event**: Single-signal synchronization primitive
//! - **WaitQueue**: Queue for threads waiting on a condition
//!
//! # Design
//!
//! All primitives are designed to work with minimal dependencies and provide
//! proper integration for future scheduler integration.

pub mod spinlock;
pub mod event;
pub mod wait_queue;

// Re-exports
pub use spinlock::{SpinMutex, SpinMutexGuard, SpinLock, SpinLockGuard};
pub use event::{Event as SyncEvent, EventFlags as SyncEventFlags};
pub use wait_queue::{WaitQueue, WaitQueueEntry, WaiterId, WaitStatus, WAIT_OK, WAIT_TIMED_OUT};
