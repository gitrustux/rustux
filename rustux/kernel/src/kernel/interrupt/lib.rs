// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! Interrupt controller module - architecture-independent interface

#![no_std]

// Re-export interrupt controller
pub mod controller;

pub use controller::{InterruptController, X86_64InterruptController};
