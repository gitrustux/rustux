// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel Objects & IPC (Zircon-style)
//!
//! This module implements the capability-based kernel object model
//! inspired by Zircon. All kernel resources are accessed through
//! handles with rights, ensuring fine-grained access control.
//!
//! # Design
//!
//! - **Capability-based security**: All operations through handles with rights
//! - **Object types**: Process, Thread, VMO, VMAR, Channel, Event, Timer, Job, Port
//! - **Handle passing**: IPC can transfer handles with rights reduction
//! - **Reference counting**: Automatic cleanup when last handle is closed
//!
//! # Modules
//!
//! - [`handle`] - Handle and rights model
//! - [`vmo`] - Virtual Memory Objects
//! - [`channel`] - IPC channels
//! - [`event`] - Event objects
//! - [`timer`] - Timer objects
//! - [`job`] - Job objects (resource containers)

pub mod handle;
pub mod vmo;
pub mod channel;
pub mod event;
pub mod timer;
pub mod job;

// Re-exports
pub use handle::{
    Handle, HandleId, HandleOwner, HandleTable, KernelObjectBase, Rights, ObjectType,
    HandleEntry, MAX_HANDLES,
};
pub use job::{Job, JobId, JobPolicy, ResourceLimits, JobStats, JOB_ID_ROOT, JOB_ID_INVALID};
pub use event::{Event, EventId, EventFlags};
pub use timer::{Timer, TimerId, TimerState, SlackPolicy};
pub use channel::{Channel, ChannelId, ChannelState, Message, ReadResult, MAX_MSG_SIZE, MAX_MSG_HANDLES};
pub use vmo::{Vmo, VmoId, VmoFlags, CachePolicy};
