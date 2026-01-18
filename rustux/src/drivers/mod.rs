// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Device Drivers
//!
//! This module contains device drivers for the Rustux kernel.
//! Drivers are organized by device type and architecture.

/// UART (serial) drivers
pub mod uart;

// Re-exports
pub use uart::{Uart16550, COM1_PORT, COM2_PORT, COM3_PORT, COM4_PORT, init_com1, com1};
