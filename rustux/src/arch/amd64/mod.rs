// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 (amd64) architecture-specific code
//!
//! This module contains all x86_64-specific implementations,
//! including the APIC (Local APIC + I/O APIC) interrupt controller.

pub mod apic;
pub mod controller;
pub mod idt;
pub mod descriptor;
pub mod mm;
pub mod mmu;
pub mod init;

#[cfg(feature = "kernel_test")]
pub mod test;

pub use controller::X86_64InterruptController;
