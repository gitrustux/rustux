// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Testing infrastructure for interrupt verification
//!
//! This module provides testing utilities for verifying interrupt functionality.
//!
//! # Usage
//! ```ignore
//! use rustux::testing::InterruptTestHarness;
//!
//! let mut harness = InterruptTestHarness::new();
//! harness.test_ioapic_discovery();
//! harness.test_irq_routing(1, 33);
//! ```

pub mod harness;
pub mod qemu;

pub use harness::InterruptTestHarness;
pub use qemu::QemuTestConfig;
