// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Architecture-specific modules
//!
//! This module provides organization for architecture-specific code.
//! Each architecture (amd64, arm64, riscv64) has its own subdirectory
//! with architecture-specific implementations of common interfaces.

pub mod amd64;
pub mod arm64;
pub mod riscv64;

// Re-export the interrupt controllers for each architecture
pub use amd64::X86_64InterruptController;
pub use arm64::Arm64InterruptController;
pub use riscv64::Riscv64InterruptController;
