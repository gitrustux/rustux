// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! Architecture-specific kernel code

#![no_std]

// Re-export architecture-specific modules
pub mod amd64;
pub mod arm64;
pub mod riscv64;

// Export commonly used types
pub use crate::kernel::types::*;
