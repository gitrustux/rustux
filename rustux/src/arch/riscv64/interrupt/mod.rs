// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V Interrupt Support
//!
//! This module provides interrupt support for RISC-V systems using:
//! - PLIC (Platform-Level Interrupt Controller) for external interrupts
//! - CLINT (Core-Local Interrupt Controller) for timer and software interrupts

pub mod plic;

// Re-exports
pub use plic::{Plic, PlicHartContext, PlicIrq, PlicPriority};

