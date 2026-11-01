// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Processes and threads.
//!
//! The kernel objects (jobs, processes, threads) live in `kobj::task`. This
//! module holds the scheduler side:
//!
//! - `table`: one entry per running thread (kernel stack, saved state)
//! - `run`: switching, blocking, waking, killing, exiting
//! - `launch`: creating processes, starting threads, loading ELF programs
//! - `switch`: older context-switch helpers

pub mod table;
pub mod switch;
pub mod run;
pub mod launch;
