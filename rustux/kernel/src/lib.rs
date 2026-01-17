// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! Rustux Kernel Library
//!
//! This is the main library for the Rustux kernel, containing architecture-independent
//! kernel functionality and core abstractions.

#![no_std]
#![no_main]

// Architecture support
pub mod kernel;
pub mod interrupt;

// Core kernel modules
pub mod sched;
pub mod mm;
pub mod process;
