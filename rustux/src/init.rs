// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel Initialization
//!
//! This module provides kernel initialization functions for the Rustux kernel.
//! It coordinates the initialization of various kernel subsystems.
//!
//! # Initialization Order
//!
//! The kernel must be initialized in a specific order:
//!
//! 1. Early architecture setup (arch, interrupts, MMU)
//! 2. Physical memory manager
//! 3. Virtual memory subsystem
//! 4. Per-CPU data
//! 5. Thread subsystem
//! 6. Scheduler
//! 7. Timer subsystem
//! 8. Syscall layer
//!
//! # Usage
//!
//! ```rust
//! // Called from architecture-specific boot code
//! kernel_init();
//! ```

// ============================================================================
// Initialization State
// ============================================================================

/// Kernel initialization state
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
pub enum InitState {
    /// Not initialized
    NotStarted = 0,

    /// Early initialization in progress
    Early = 1,

    /// Architecture-specific initialization
    Arch = 2,

    /// Physical memory manager initialized
    PMM = 3,

    /// Virtual memory initialized
    VM = 4,

    /// Per-CPU data initialized
    PerCpu = 5,

    /// Thread subsystem initialized
    Thread = 6,

    /// Scheduler initialized
    Scheduler = 7,

    /// Timer subsystem initialized
    Timer = 8,

    /// Syscall layer initialized
    Syscall = 9,

    /// Late initialization complete
    Complete = 10,

    /// Running (initialization done)
    Running = 11,
}

/// Current initialization state
static mut INIT_STATE: InitState = InitState::NotStarted;

/// ============================================================================
/// Public API
/// ============================================================================

/// Initialize the kernel
///
/// This is the main kernel initialization function.
/// It should be called from architecture-specific boot code.
///
/// # Safety
///
/// Must be called exactly once during kernel boot.
pub fn kernel_init() {
    unsafe {
        if INIT_STATE != InitState::NotStarted {
            panic!("kernel_init called multiple times");
        }
        INIT_STATE = InitState::Early;
    }

    // Initialize subsystems in order
    init_early();
    init_arch();
    init_memory();
    init_threads();
    init_late();

    unsafe {
        INIT_STATE = InitState::Complete;
    }
}

/// Get the current initialization state
pub fn init_state() -> InitState {
    unsafe { INIT_STATE }
}

/// ============================================================================
/// Initialization Phases
/// ============================================================================

/// Early initialization
///
/// Initializes core subsystems needed for everything else.
fn init_early() {
    // TODO: Initialize debug/logging
    // TODO: Initialize command line parsing
    // TODO: Initialize platform-specific code
    unsafe {
        INIT_STATE = InitState::Early;
    }
}

/// Architecture-specific initialization
///
/// Initializes architecture-specific hardware interfaces.
fn init_arch() {
    // Call the architecture-specific init function
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::amd64::init::arch_init();
    }

    #[cfg(target_arch = "aarch64")]
    {
        // TODO: crate::arch::arm64::init();
    }

    #[cfg(target_arch = "riscv64")]
    {
        // TODO: crate::arch::riscv64::init();
    }

    unsafe {
        INIT_STATE = InitState::Arch;
    }
}

/// Memory subsystem initialization
///
/// Initializes physical and virtual memory management.
fn init_memory() {
    // TODO: Initialize physical memory manager
    // TODO: Initialize virtual memory subsystem
    // TODO: Initialize kernel stack allocator

    unsafe {
        INIT_STATE = InitState::VM;
    }
}

/// Thread and scheduler initialization
///
/// Initializes the threading and scheduling subsystems.
fn init_threads() {
    // TODO: Initialize thread subsystem
    // TODO: Initialize scheduler

    unsafe {
        INIT_STATE = InitState::Scheduler;
    }
}

/// Late initialization
///
/// Initializes remaining subsystems.
fn init_late() {
    // TODO: Initialize syscall layer
    // TODO: User/kernel boundary safety

    unsafe {
        INIT_STATE = InitState::Complete;
    }
}

/// Mark kernel as running
///
/// Called after all initialization is complete.
pub fn kernel_running() {
    unsafe {
        INIT_STATE = InitState::Running;
    }

    // TODO: Create idle thread for CPU 0
    // TODO: Start scheduler

    // For now, just halt
    loop {}
}

/// Idle thread entry point
///
/// This is the entry point for idle threads.
/// When there's no work to do, the idle thread runs.
pub extern "C" fn idle_thread_entry(_cpu_id: usize) -> ! {
    // TODO: Implement proper idle loop
    loop {
        // TODO: Check for pending work
        // If no work, halt the CPU until interrupt
        // Repeat

        // For now, just spin
        core::hint::spin_loop();
    }
}
