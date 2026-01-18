// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! # Rustux - A Zircon-inspired Kernel in Rust
//!
//! Rustux is a microkernel project inspired by Zircon (Fuchsia's kernel),
//! implemented in Rust. It aims to provide:
//!
//! - **Multi-architecture support**: x86_64 (APIC), ARM64 (GIC), RISC-V (PLIC)
//! - **Capability-based security**: Following Zircon's object model
//! - **Clean architecture**: Separation of architecture-specific and generic code
//! - **Memory safety**: Leveraging Rust's type system for kernel safety
//!
//! ## Architecture
//!
//! The kernel is organized with clear separation between architecture-specific
//! and architecture-independent code:
//!
//! ```text
//! src/
//! ├── arch/              # Architecture-specific code
//! │   ├── amd64/         # x86_64 APIC implementation
//! │   ├── arm64/         # ARM GIC implementation (TODO)
//! │   └── riscv64/       # RISC-V PLIC implementation (TODO)
//! ├── interrupt/         # Generic interrupt handling
//! ├── drivers/           # Device drivers
//! └── lib.rs            # This file
//! ```
//!
//! ## Interrupt Controller Abstraction
//!
//! Each architecture implements the [`InterruptController`] trait:
//!
//! - **x86_64**: Uses Local APIC + I/O APIC
//! - **ARM64**: Uses GIC (Generic Interrupt Controller) - TODO
//! - **RISC-V**: Uses PLIC (Platform-Level Interrupt Controller) - TODO
//!
//! ## Status
//!
//! - ✅ Cross-architecture interrupt controller trait
//! - ✅ x86_64 APIC implementation (Local APIC + IOAPIC)
//! - ⚠️ ARM64 GIC implementation (pending)
//! - ⚠️ RISC-V PLIC implementation (pending)
//! - ⚠️ ACPI MADT parsing for dynamic APIC discovery (pending)
//!
//! ## Using the Interrupt Controller
//!
//! ```ignore
//! use rustux::arch::X86_64InterruptController;
//! use rustux::traits::InterruptController;
//!
//! let mut controller = X86_64InterruptController::new();
//! controller.init().unwrap();
//! controller.enable_irq(1, 33); // Route IRQ1 to vector 33
//! ```

#![no_std]
#![feature(abi_x86_interrupt)]

// Core traits and types
pub mod traits;

// Architecture-specific modules
pub mod arch;

// Generic interrupt handling
pub mod interrupt;

// ACPI table parsing
pub mod acpi;

// Testing infrastructure
#[cfg(test)]
pub mod testing;

// Test kernel entry point (for QEMU testing)
#[cfg(feature = "kernel_test")]
pub mod test_entry;

// Scheduler and thread management
pub mod sched;

// Kernel initialization
pub mod init;

// Re-export commonly used types
pub use traits::{
    InterruptController,
    InterruptTriggerMode,
    InterruptPolarity,
    InterruptDeliveryMode,
};

// Re-export architecture-specific controllers
pub use arch::{
    X86_64InterruptController,
    Arm64InterruptController,
    Riscv64InterruptController,
};

// Re-export interrupt handler
pub use interrupt::{
    InterruptHandler,
    X86_64InterruptHandler,
    Arm64InterruptHandler,
    Riscv64InterruptHandler,
};

// Re-export ACPI types
pub use acpi::{
    Rsdp,
    find_rsdp,
    find_and_parse_madt,
    ParsedMadt,
    IoApicEntry,
    LocalApicEntry,
};

// Re-export testing types
#[cfg(test)]
pub use testing::{
    InterruptTestHarness,
    QemuTestConfig,
};

// Re-export scheduler types
pub use sched::{
    Thread,
    ThreadId,
    EntryPoint,
    Scheduler,
    SchedulingPolicy,
    ThreadState,
    ThreadPriority,
};

// Note: Panic handler is provided by the binary (main.rs) when building as a kernel.
// When using this library as a dependency, users must provide their own panic handler.
