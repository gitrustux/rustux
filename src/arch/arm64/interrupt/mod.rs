// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ARM64 Interrupt Support
//!
//! This module provides interrupt support for ARM64 systems using the
//! Generic Interrupt Controller (GIC).

pub mod gic;

// Re-exports
pub use gic::{GicV2, GicV3, GicVersion, GicInfo, gicd_offset, gicc_offset};
