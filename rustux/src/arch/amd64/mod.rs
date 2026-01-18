// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86_64 (amd64) architecture-specific code
//!
//! This module contains all x86_64-specific implementations,
//! including the APIC (Local APIC + I/O APIC) interrupt controller.

// APIC and interrupt controller
pub mod apic;
pub mod controller;

// Descriptor tables (GDT, IDT)
pub mod idt;
pub mod descriptor;

// Memory management
pub mod mm;
pub mod mmu;

// System initialization and testing
pub mod init;

#[cfg(feature = "kernel_test")]
pub mod test;

// Low-level CPU operations
pub mod registers;
pub mod tsc;
pub mod ioport;
pub mod cache;
pub mod ops;

// System call support
pub mod syscall;

// User space entry
pub mod uspace_entry;

// Kernel to userspace transition (mexec)
pub mod mexec;

// Exception and fault handlers
pub mod faults;

// Bootstrap support for SMP
pub mod bootstrap16;

// Re-export the interrupt controller
pub use controller::X86_64InterruptController;
