// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! # Rustux - A Capability-based Microkernel in Rust
//!
//! Rustux is a microkernel project implemented in Rust. It aims to provide:
//!
//! - **Multi-architecture support**: x86_64 (APIC), ARM64 (GIC), RISC-V (PLIC)
//! - **Capability-based security**: Object-based resource access control
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
//! ├── mm/                # Memory management (PMM, heap allocator)
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
//! - ✅ Physical Memory Manager (PMM) with bitmap allocator
//! - ✅ Heap allocator with linked list implementation
//! - ✅ Page table management for x86_64
//! - ⚠️ ARM64 GIC implementation (pending)
//! - ⚠️ RISC-V PLIC implementation (pending)
//! - ⚠️ Virtual Memory Manager (VMM) (in progress)
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

// Alloc crate for heap allocations
extern crate alloc;

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

// System call interface
pub mod syscall;

// Memory management
pub mod mm;

// Synchronization primitives
pub mod sync;

// Process management
pub mod process;

// Filesystem
pub mod fs;

// Device drivers
pub mod drivers;

// Execution and ELF loading
pub mod exec;

// Kernel objects (capability-based security)
pub mod object;

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

// Re-export memory management types
pub use mm::{
    // PMM types and functions
    PageState,
    ArenaInfo,
    Page,
    pmm_add_arena,
    pmm_alloc_page,
    pmm_alloc_kernel_page,
    pmm_alloc_user_page,
    pmm_alloc_contiguous,
    pmm_free_page,
    pmm_free_contiguous,
    pmm_count_free_pages,
    pmm_count_total_pages,
    pmm_count_total_bytes,
    pmm_init_early,
    set_boot_allocator,
    // Zone constants
    KERNEL_ZONE_START,
    KERNEL_ZONE_END,
    USER_ZONE_START,
    USER_ZONE_END,
    // Page utilities
    PAGE_SIZE,
    is_page_aligned,
    align_page_down,
    align_page_up,
    bytes_to_pages,
    pages_to_bytes,
    // Heap allocator
    heap_init,
    heap_init_aligned,
    heap_allocate,
    heap_deallocate,
    heap_usage,
    heap_size,
    heap_available,
};

// Re-export synchronization types
pub use sync::{
    SpinMutex, SpinMutexGuard, SpinLock, SpinLockGuard,
    SyncEvent,
    SyncEventFlags,
    WaitQueue, WaitQueueEntry, WaiterId, WaitStatus,
    WAIT_OK, WAIT_TIMED_OUT,
};

// Re-export process types
pub use process::{
    ProcessId, PID_INVALID, PID_KERNEL, PID_FIRST_USER,
    ProcessState,
    AddressSpace,
    ProcessFlags,
    ReturnCode,
    Process, MAX_THREADS_PER_PROCESS,
};

// Re-export driver types
pub use drivers::{
    Uart16550, COM1_PORT, COM2_PORT, COM3_PORT, COM4_PORT, init_com1, com1,
};

// Re-export kernel object types
pub use object::{
    // Handle & rights
    Handle, HandleId, HandleOwner, HandleTable, KernelObjectBase, Rights, ObjectType,
    HandleEntry, MAX_HANDLES,
    // Job
    Job, JobId, JobPolicy, ResourceLimits, JobStats, JOB_ID_ROOT, JOB_ID_INVALID,
    // Event
    Event, EventId, EventFlags,
    // Timer
    Timer, TimerId, TimerState, SlackPolicy,
    // Channel
    Channel, ChannelId, ChannelState, Message, ReadResult, MAX_MSG_SIZE, MAX_MSG_HANDLES,
    // VMO
    Vmo, VmoId, VmoFlags, CachePolicy,
};

// Integration tests (only compiled in test mode)
#[cfg(test)]
mod tests;

// Note: Panic handler is provided by the binary (main.rs) when building as a kernel.
// When using this library as a dependency, users must provide their own panic handler.
