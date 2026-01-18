// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Scheduler and thread management
//!
//! This module provides basic scheduler primitives for managing threads.
//!
//! # Example
//! ```ignore
//! use rustux::sched::{Scheduler, Thread};
//!
//! let mut scheduler = Scheduler::new();
//! let thread = Thread::new(entry_point, stack_base);
//! scheduler.add_thread(thread);
//! scheduler.schedule();
//! ```

pub mod thread;
pub mod scheduler;
pub mod state;

pub use thread::{Thread, ThreadId, EntryPoint};
pub use scheduler::{Scheduler, SchedulingPolicy};
pub use state::{ThreadState, RunQueue, ThreadPriority};
