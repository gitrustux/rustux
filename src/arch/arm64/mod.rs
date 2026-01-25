// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ARM64 architecture-specific code
//!
//! This module contains all ARM64-specific implementations.
//!
//! # Modules
//!
//! - [`arch`] - Architecture definitions and CPU features
//! - [`interrupt`] - GIC (Generic Interrupt Controller) support
//! - [`mm`] - Memory management unit (MMU) and page tables

pub mod arch;
pub mod interrupt;
pub mod mm;

// Re-exports
pub use arch::{Arm64ArchInfo, Arm64Features, Arm64SpInfo, Arm64InterruptController, ARM64_MAX_CPUS, ARM64_PAGE_SIZE};
pub use interrupt::{GicV2, GicV3, GicVersion, GicInfo};
pub use mm::{PAddr};
