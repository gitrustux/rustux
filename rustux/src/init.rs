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

use core::sync::atomic::{AtomicUsize, Ordering};
use crate::arch::amd64::mmu::PAddr;

const QEMU_DEBUGCON_PORT: u16 = 0xE9;

fn qemu_debugcon_write_byte(b: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") QEMU_DEBUGCON_PORT, in("al") b, options(nostack, nomem));
    }
}

fn debug_print(s: &str) {
    for byte in s.bytes() {
        qemu_debugcon_write_byte(byte);
    }
}

fn print_hex(mut n: u64) {
    if n == 0 {
        qemu_debugcon_write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 16];
    let mut i = 0;
    while n > 0 {
        let digit = (n % 16) as u8;
        buf[i] = if digit < 10 { b'0' + digit } else { b'A' + digit - 10 };
        n /= 16;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        qemu_debugcon_write_byte(buf[i]);
    }
}

/// Boot allocator - simple bump allocator for early boot
///
/// Uses a static buffer to provide memory for PMM initialization.
/// This is needed because PMM needs memory for its structures before it can allocate.
struct BootAllocator {
    start: AtomicUsize,
    size: usize,
    offset: AtomicUsize,
}

impl BootAllocator {
    const fn new(size: usize) -> Self {
        Self {
            start: AtomicUsize::new(0),
            size,
            offset: AtomicUsize::new(0),
        }
    }

    unsafe fn init(&self, start: usize) {
        self.start.store(start, Ordering::Release);
    }

    unsafe fn alloc(&self, size: usize, align: usize) -> *mut u8 {
        let base = self.start.load(Ordering::Acquire);
        let current = self.offset.load(Ordering::Acquire);

        // Align the offset
        let aligned = if current % align == 0 {
            current
        } else {
            ((current / align) + 1) * align
        };

        let new_offset = aligned + size;

        if new_offset > self.size {
            return core::ptr::null_mut();
        }

        if self.offset.compare_exchange(current, new_offset, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            (base + aligned) as *mut u8
        } else {
            // Retry if there was a race (shouldn't happen in single-threaded boot)
            self.alloc(size, align)
        }
    }
}

/// Static boot allocator buffer
/// 2MB for PMM page structures (Vec<Page> with ~32 bytes per page)
/// For 126MB of memory: 32,256 pages * 32 bytes = ~1MB, use 2MB for safety
static mut BOOT_ALLOC_BUFFER: [u8; 2 * 1024 * 1024] = [0; 2 * 1024 * 1024];

static BOOT_ALLOCATOR: BootAllocator = BootAllocator::new(2 * 1024 * 1024);

/// Boot allocator callback for PMM
unsafe extern "C" fn boot_alloc_callback(size: usize, align: usize) -> *mut u8 {
    BOOT_ALLOCATOR.alloc(size, align)
}

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
    unsafe {
        use crate::mm::pmm;

        // First, initialize the boot allocator with the buffer address
        BOOT_ALLOCATOR.init(BOOT_ALLOC_BUFFER.as_ptr() as usize);

        // Debug print
        let msg = b"[INIT] Boot allocator initialized\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Set up the boot allocator for PMM
        pmm::set_boot_allocator(boot_alloc_callback);

        // Debug print
        let msg = b"[INIT] Calling pmm_init_early...\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Initialize PMM - we need memory allocation before anything else
        //
        // PHYSICAL MEMORY ZONING (Fix #1):
        // Split physical memory into separate zones to prevent VMO clone
        // operations from corrupting kernel heap metadata.
        //
        // Kernel Zone: 0x00200000 - 0x00FFFFFF (14 MB)
        //   - Kernel heap
        //   - Page tables
        //   - Kernel metadata structures
        //
        // User Zone: 0x01000000 - 0x07FE0000 (112 MB)
        //   - VMO backing pages
        //   - User data
        //   - Clone destinations
        //
        const KERNEL_ZONE_BASE: u64 = 0x0020_0000;   // 2MB (after kernel image)
        const KERNEL_ZONE_SIZE: usize = 14 * 1024 * 1024;  // 14MB
        const USER_ZONE_BASE: u64 = 0x0100_0000;    // 16MB
        const USER_ZONE_SIZE: usize = 112 * 1024 * 1024;  // 112MB

        // Add kernel zone arena
        let kernel_info = pmm::ArenaInfo::new(
            b"kernel\0\0\0\0\0\0\0\0\0\0",
            pmm::ARENA_FLAG_LOW_MEM | pmm::ARENA_FLAG_KERNEL,
            0, // highest priority
            KERNEL_ZONE_BASE,
            KERNEL_ZONE_SIZE,
        );
        let _ = pmm::pmm_add_arena(kernel_info);

        // Add user zone arena
        let user_info = pmm::ArenaInfo::new(
            b"user\0\0\0\0\0\0\0\0\0\0\0\0",
            pmm::ARENA_FLAG_LOW_MEM | pmm::ARENA_FLAG_USER,
            1, // lower priority
            USER_ZONE_BASE,
            USER_ZONE_SIZE,
        );
        let _ = pmm::pmm_add_arena(user_info);

        // Debug print
        let msg = b"[INIT] PMM init complete, free pages: \n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Print number of free pages
        let free_pages = pmm::pmm_count_free_pages();
        print_hex(free_pages);

        let msg = b"\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

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
    // Initialize the heap allocator
    // Use a simple heap in the kernel's BSS
    extern crate alloc;

    #[cfg(target_arch = "x86_64")]
    {
        use crate::mm::pmm;

        unsafe {
            // Debug print before heap init
            let msg = b"[INIT] Starting heap initialization...\n";
            for &byte in msg {
                core::arch::asm!(
                    "out dx, al",
                    in("dx") 0xE9u16,
                    in("al") byte,
                    options(nomem, nostack)
                );
            }

            // TODO: Get actual memory map from UEFI
            // WORKAROUND: PMM has a bug where it only allocates 1 page
            // Use a hardcoded physical address for the heap
            // In QEMU with 128MB RAM, physical address 0x1000000 (16MB) should be safe
            // CRITICAL FIX: Heap must be in KERNEL zone, not USER zone!
            // The heap contains kernel metadata (VMO BTreeMaps, etc.) and must not
            // be in the same physical region as user-visible memory (VMO backing pages).
            //
            // KERNEL_ZONE: 0x00200000 - 0x00FFFFFF (14 MB)
            // USER_ZONE:   0x01000000 - 0x7FFFFFFF (2 GB+)
            //
            // Previous heap at 0x1000000 was in USER zone, causing corruption!
            // New heap at 0x00300000 is safely in KERNEL zone.
            const HEAP_PADDR: u64 = 0x0030_0000;  // 3MB physical address (in KERNEL zone)
            const HEAP_SIZE: usize = 1024 * 1024; // 1MB heap

            let heap_start_vaddr = pmm::paddr_to_vaddr(HEAP_PADDR);

            let msg = b"[INIT] Using hardcoded heap at 0x";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            print_hex(heap_start_vaddr as u64);
            let msg = b", size: 0x100000 (1MB)\n[INIT] Initializing heap...\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }

            // Initialize the heap
            crate::mm::heap_init_aligned(heap_start_vaddr as usize, HEAP_SIZE);

            // Reserve the heap pages in the PMM so they won't be allocated for other uses
            // Heap size: 1MB = 256 pages
            const HEAP_PAGES: usize = 256;
            let _ = pmm::pmm_reserve_pages(HEAP_PADDR, HEAP_PAGES);

            let msg = b"[INIT] Heap initialized successfully (1MB)\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }

            INIT_STATE = InitState::VM;
        }
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // TODO: Implement for other architectures
        unsafe {
            INIT_STATE = InitState::VM;
        }
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

    // DEBUG: Prove we reached init_late
    unsafe {
        let msg = b"[INIT] Reached init_late()\n";
        for &b in msg {
            core::arch::asm!("out dx, al",
                in("dx") 0xE9u16,
                in("al") b,
                options(nomem, nostack));
        }
    }

    // Test userspace execution (Phase 4A)
    #[cfg(feature = "userspace_test")]
    {
        // DEBUG: Before userspace_exec_test call
        unsafe {
            let msg = b"[INIT] BEFORE userspace_exec_test call\n";
            for &b in msg {
                core::arch::asm!("out dx, al",
                    in("dx") 0xE9u16,
                    in("al") b,
                    options(nomem, nostack));
            }
        }

        unsafe {
            crate::exec::userspace_exec_test::test_userspace_execution();
        }

        // DEBUG: After userspace_exec_test call (should never reach here)
        unsafe {
            let msg = b"[INIT] AFTER userspace_exec_test call (UNREACHABLE)\n";
            for &b in msg {
                core::arch::asm!("out dx, al",
                    in("dx") 0xE9u16,
                    in("al") b,
                    options(nomem, nostack));
            }
        }
    }

    #[cfg(not(feature = "userspace_test"))]
    {
        // DEBUG: Feature gate not enabled
        unsafe {
            let msg = b"[INIT] userspace_test feature NOT enabled - skipping test\n";
            for &b in msg {
                core::arch::asm!("out dx, al",
                    in("dx") 0xE9u16,
                    in("al") b,
                    options(nomem, nostack));
            }
        }
    }

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

/// Get the kernel PML4 virtual address for temporary mappings
///
/// This is used by VMO copy operations to access the kernel page tables.
/// For now, we use CR3 to get the current PML4 physical address and convert it.
///
/// # Safety
///
/// Must be called after kernel page tables are set up.
pub unsafe fn get_kernel_pml4_vaddr() -> crate::arch::amd64::mm::VAddr {
    // Read CR3 to get PML4 physical address
    let cr3: u64;
    core::arch::asm!(
        "mov {}, cr3",
        out(reg) cr3,
        options(nomem, nostack)
    );

    // Convert physical address to virtual (identity mapping for low memory)
    (cr3 as crate::arch::amd64::mm::VAddr)
}

