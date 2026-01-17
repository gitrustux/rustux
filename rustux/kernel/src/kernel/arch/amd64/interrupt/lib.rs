// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! AMD64 interrupt controller support

#![no_std]

// Export interrupt controller
pub mod controller;
pub use controller::{InterruptController, X86_64InterruptController};
