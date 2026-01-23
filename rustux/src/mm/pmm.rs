// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Physical Memory Manager (PMM) - Vec-Based Implementation
//!
//! This is a simplified PMM using a Vec-based page array with linear scan allocation.
//! This replaces the bitmap-based PMM which had a bug in the atomic bitmap handling.
//!
//! # Architecture Note
//!
//! This represents the "Early Kernel PMM" stage - simple, reliable, proven to work.
//!
//! # Design
//!
//! - Uses a Vec of Page structures with state tracking
//! - Linear scan allocation (O(N) where N = total pages)
//! - No atomic operations (single-threaded initially)
//! - Simple state enum: Free | Allocated | Reserved
//!
//! # Migration Path
//!
//! Phase A (NOW): Use this Vec-based PMM to unblock userspace
//! Phase B (LATER): Reintroduce bitmap PMM with proper testing as optimization
//!
//! # Usage
//!
//! ```rust
//! // Allocate a single page
//! let page = pmm::pmm_alloc_page(0)?;
//!
//! // Free a page
//! pmm::pmm_free_page(page);
//!
//! // Count free pages
//! let free = pmm::pmm_count_free_pages();
//! ```

use crate::arch::amd64::mm::{
    PAddr,
    VAddr,
    RxStatus,
    RxResult,
    page_tables::PAGE_SIZE
};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Global PMM allocation call counter
static ALLOC_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Helper: Print decimal number to debug console
unsafe fn print_decimal(mut n: usize) {
    if n == 0 {
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b'0', options(nomem, nostack));
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        let digit = (n % 10) as u8;
        buf[i] = b'0' + digit;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
    }
}

/// Page size shift for quick division/multiplication
pub const PAGE_SIZE_SHIFT: u8 = 12;

/// Mask for page-aligned addresses
pub const PAGE_MASK: usize = PAGE_SIZE - 1;

/// Maximum number of physical memory arenas
const MAX_ARENAS: usize = 8;

/// Check if an address is page-aligned
#[inline]
pub const fn is_page_aligned(addr: usize) -> bool {
    (addr & PAGE_MASK) == 0
}

/// Align an address down to page boundary
#[inline]
pub const fn align_page_down(addr: usize) -> usize {
    addr & !PAGE_MASK
}

/// Align an address up to page boundary
#[inline]
pub const fn align_page_up(addr: usize) -> usize {
    (addr + PAGE_MASK) & !PAGE_MASK
}

/// Convert bytes to number of pages (rounding up)
#[inline]
pub const fn bytes_to_pages(bytes: usize) -> usize {
    (bytes + PAGE_MASK) / PAGE_SIZE
}

/// Convert pages to bytes
#[inline]
pub const fn pages_to_bytes(pages: usize) -> usize {
    pages << PAGE_SIZE_SHIFT
}

/// Page state enumeration
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// Page is free and can be allocated
    Free = 0,

    /// Page is allocated (general purpose)
    Allocated = 1,

    /// Page is reserved (cannot be allocated)
    Reserved = 2,
}

impl PageState {
    /// Check if this page is free
    pub fn is_free(&self) -> bool {
        *self == PageState::Free
    }
}

/// Physical memory arena information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ArenaInfo {
    /// Arena name (e.g., "low_mem", "high_mem")
    pub name: [u8; 16],

    /// Arena flags (e.g., LOW_MEM, HIGH_MEM)
    pub flags: u32,

    /// Arena allocation priority (lower = higher priority)
    pub priority: u32,

    /// Base physical address
    pub base: PAddr,

    /// Size in bytes
    pub size: usize,
}

impl ArenaInfo {
    /// Create a new arena info structure
    pub const fn new(name: &[u8], flags: u32, priority: u32, base: PAddr, size: usize) -> Self {
        let mut name_bytes = [0u8; 16];
        let mut i = 0;
        while i < 16 && i < name.len() {
            name_bytes[i] = name[i];
            i += 1;
        }

        Self {
            name: name_bytes,
            flags,
            priority,
            base,
            size,
        }
    }

    /// Get the number of pages in this arena
    pub fn page_count(&self) -> usize {
        self.size / PAGE_SIZE
    }

    /// Get the end physical address (exclusive)
    pub fn end(&self) -> PAddr {
        self.base + self.size as PAddr
    }
}

/// Arena allocation flags
pub const ARENA_FLAG_LOW_MEM: u32 = 0x1;   // Arena is in "low memory" (< 4GB)
pub const ARENA_FLAG_HIGH_MEM: u32 = 0x2;  // Arena is in "high memory" (>= 4GB)
pub const ARENA_FLAG_KERNEL: u32 = 0x4;   // Arena is for kernel metadata (page tables, heap)
pub const ARENA_FLAG_USER: u32 = 0x8;     // Arena is for user memory (VMO backing pages)

/// PMM allocation flags
pub const PMM_ALLOC_FLAG_ANY: u32 = 0x0;       // Allocate from any arena
pub const PMM_ALLOC_FLAG_LOW_MEM: u32 = 0x1;   // Allocate only from low memory arenas
pub const PMM_ALLOC_FLAG_KERNEL: u32 = 0x4;    // Allocate from kernel zone only
pub const PMM_ALLOC_FLAG_USER: u32 = 0x8;      // Allocate from user zone only

/// Memory zone definitions
///
/// These define the physical memory regions for kernel vs user allocations.
/// This prevents VMO clone operations from corrupting kernel heap metadata.
///
/// # Important Invariant
///
/// Kernel metadata (heap, page tables, allocator structures) must NEVER
/// live in the same physical region as user-visible memory (VMO backing pages).
///
/// Kernel Zone: 0x00100000 - 0x00FFFFFF (16 MB)
///   - Kernel heap
///   - Page tables
///   - Kernel metadata structures
///
/// User Zone: 0x01000000 - 0x7FFFFFFF (2 GB+)
///   - VMO backing pages
///   - User data
///   - Clone destinations
pub const KERNEL_ZONE_START: PAddr = 0x0010_0000;  // 1 MB
pub const KERNEL_ZONE_END: PAddr = 0x00FF_FFFF;    // 16 MB
pub const USER_ZONE_START: PAddr = 0x0100_0000;    // 16 MB
pub const USER_ZONE_END: PAddr = 0x7FFF_FFFF;      // 2 GB

/// Page structure tracking a single physical page
#[repr(C)]
#[derive(Debug)]
pub struct Page {
    /// Physical address of this page
    pub paddr: PAddr,

    /// Current state of the page
    pub state: PageState,

    /// Reference count for shared pages
    pub ref_count: u32,

    /// Arena index this page belongs to
    pub arena_index: u8,

    /// Page index within arena
    pub page_index: u32,
}

impl Page {
    /// Create a new page structure
    pub const fn new(paddr: PAddr, arena_index: u8, page_index: u32) -> Self {
        Self {
            paddr,
            state: PageState::Free,
            ref_count: 0,
            arena_index,
            page_index,
        }
    }

    /// Check if this page is free
    pub fn is_free(&self) -> bool {
        self.state.is_free()
    }
}

/// Memory arena structure
struct Arena {
    /// Arena information
    info: ArenaInfo,

    /// Array of page structures
    pages: alloc::vec::Vec<Page>,

    /// Total number of pages
    total_count: u64,

    /// Lock for arena operations (spinlock for now)
    locked: core::sync::atomic::AtomicBool,
}

impl Arena {
    /// Create a new arena
    const fn new(info: ArenaInfo) -> Self {
        Self {
            info,
            pages: alloc::vec::Vec::new(),
            total_count: 0,
            locked: core::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Initialize the arena with page structures
    fn init(&mut self, pages: alloc::vec::Vec<Page>) {
        self.total_count = pages.len() as u64;
        self.pages = pages;
    }

    /// Allocate a single page from this arena
    fn alloc_page(&mut self) -> Option<PAddr> {
        // Simple linear scan for free page
        for page in &mut self.pages {
            if page.is_free() {
                page.state = PageState::Allocated;
                page.ref_count = 1;
                return Some(page.paddr);
            }
        }
        None
    }

    /// Free a page back to this arena
    fn free_page(&mut self, paddr: PAddr) -> RxStatus {
        let offset = paddr - self.info.base;
        if offset % PAGE_SIZE as PAddr != 0 {
            return RxStatus::ERR_INVALID_ARGS;
        }

        let index = (offset / PAGE_SIZE as PAddr) as usize;
        if index >= self.total_count as usize {
            return RxStatus::ERR_INVALID_ARGS;
        }

        self.pages[index].state = PageState::Free;
        self.pages[index].ref_count = 0;
        RxStatus::OK
    }

    /// Check if a physical address is within this arena
    fn address_in_arena(&self, addr: PAddr) -> bool {
        addr >= self.info.base && addr < (self.info.base + self.info.size as PAddr)
    }

    /// Count free pages in this arena
    fn count_free_pages(&self) -> u64 {
        self.pages.iter().filter(|p| p.is_free()).count() as u64
    }

    /// Count total pages in this arena
    fn count_total_pages(&self) -> u64 {
        self.total_count
    }
}

/// Global arena array
static mut ARENAS: [Arena; MAX_ARENAS] = [
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
    Arena::new(ArenaInfo::new(
        b"empty\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        0, 0, 0, 0
    )),
];

/// Number of arenas currently in use
static mut NUM_ARENAS: usize = 0;

/// Early PMM initialization
///
/// This function initializes the physical memory manager with memory arenas.
///
/// # Safety
///
/// Must be called exactly once during kernel boot, before any memory allocation.
pub unsafe fn pmm_init_early(
    low_base: PAddr,
    low_size: usize,
    high_base: Option<PAddr>,
    high_size: Option<usize>,
) {
    // Add low memory arena
    let low_info = ArenaInfo::new(
        b"low_mem\0",
        ARENA_FLAG_LOW_MEM,
        0, // highest priority
        low_base,
        low_size,
    );
    let _ = pmm_add_arena(low_info);

    // Add high memory arena if specified
    if let (Some(base), Some(size)) = (high_base, high_size) {
        let high_info = ArenaInfo::new(
            b"high_mem\0",
            ARENA_FLAG_HIGH_MEM,
            1, // lower priority
            base,
            size,
        );
        let _ = pmm_add_arena(high_info);
    }
}

/// Add a memory arena to the PMM
///
/// # Arguments
///
/// * `info` - Arena information (base address, size, flags)
///
/// # Returns
///
/// `RxStatus::OK` on success, or an error code
pub unsafe fn pmm_add_arena(info: ArenaInfo) -> RxStatus {
    if NUM_ARENAS >= MAX_ARENAS {
        return RxStatus::ERR_NO_MEMORY;
    }

    let page_count = info.page_count();
    if page_count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }

    // Allocate page structures array
    // For now, use the boot allocator (passed via set_boot_allocator)
    // In the future, this should use a proper boot allocator
    extern crate alloc;

    let pages_layout = core::alloc::Layout::array::<Page>(page_count).unwrap();
    let pages_ptr = if let Some(boot_alloc) = BOOT_ALLOC {
        boot_alloc(pages_layout.size(), pages_layout.align())
    } else {
        // No boot allocator configured, use heap
        // This is a workaround for testing
        use crate::mm::allocator;
        let heap_ptr = allocator::allocate(pages_layout.size(), pages_layout.align());
        if heap_ptr.is_null() {
            return RxStatus::ERR_NO_MEMORY;
        }
        heap_ptr
    };

    let pages = core::ptr::slice_from_raw_parts_mut(
        pages_ptr as *mut Page,
        page_count,
    );

    // Initialize page structures using pointer arithmetic
    let pages_slice = unsafe {
        core::slice::from_raw_parts_mut(pages_ptr as *mut Page, page_count)
    };

    for i in 0..page_count {
        pages_slice[i] = Page::new(info.base + (i as PAddr) * PAGE_SIZE as PAddr, NUM_ARENAS as u8, i as u32);
    }

    // Create Vec from the initialized memory using from_raw_parts
    // This takes ownership of the raw allocation without cloning
    let pages_vec = unsafe {
        alloc::vec::Vec::from_raw_parts(pages_ptr as *mut Page, page_count, page_count)
    };

    // Initialize arena
    let arena = &mut ARENAS[NUM_ARENAS];
    arena.info = info;
    arena.init(pages_vec);

    NUM_ARENAS += 1;
    RxStatus::OK
}

/// Boot allocator function type
type BootAllocFn = unsafe extern "C" fn(size: usize, align: usize) -> *mut u8;

/// Global boot allocator function pointer
static mut BOOT_ALLOC: Option<BootAllocFn> = None;

/// Set the boot allocator function
///
/// This must be called before pmm_add_arena to provide memory for
/// the internal data structures.
///
/// # Safety
///
/// The provided function must return valid aligned memory.
pub unsafe fn set_boot_allocator(alloc: BootAllocFn) {
    BOOT_ALLOC = Some(alloc);
}

/// Allocate a single physical page
///
/// # Arguments
///
/// * `flags` - Allocation flags (PMM_ALLOC_FLAG_*)
///
/// # Returns
///
/// Physical address of the allocated page, or an error
pub fn pmm_alloc_page(flags: u32) -> RxResult<PAddr> {
    // Increment and get call number
    let call_num = ALLOC_CALL_COUNT.fetch_add(1, Ordering::Relaxed);

    let arenas = unsafe { &mut ARENAS[..NUM_ARENAS] };

    // Debug: Log which allocator is being called WITH CALL NUMBER
    unsafe {
        let msg = b"[PMM] Call #";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        print_decimal(call_num);

        // Print allocation type separately
        if flags == PMM_ALLOC_FLAG_KERNEL {
            let msg = b" alloc_kernel_page\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        } else if flags == PMM_ALLOC_FLAG_USER {
            let msg = b" alloc_user_page\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        } else {
            let msg = b" alloc_page(GENERIC)\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }
    }

    // Try to allocate from matching arenas
    for arena in arenas {
        // Filter arenas based on requested flags
        if flags == PMM_ALLOC_FLAG_LOW_MEM && (arena.info.flags & ARENA_FLAG_LOW_MEM) == 0 {
            continue;
        }
        if flags == PMM_ALLOC_FLAG_KERNEL && (arena.info.flags & ARENA_FLAG_KERNEL) == 0 {
            continue;
        }
        if flags == PMM_ALLOC_FLAG_USER && (arena.info.flags & ARENA_FLAG_USER) == 0 {
            continue;
        }

        if let Some(paddr) = arena.alloc_page() {
            // Debug: Log SUCCESS with call number
            unsafe {
                let msg = b"[PMM] Call #";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                print_decimal(call_num);
                let msg = b" SUCCESS -> 0x";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                // Print address in hex
                let mut n = paddr;
                let mut buf = [0u8; 16];
                let mut i = 0;
                loop {
                    let digit = (n & 0xF) as u8;
                    buf[i] = if digit < 10 { b'0' + digit } else { b'a' + digit - 10 };
                    n >>= 4;
                    i += 1;
                    if n == 0 { break; }
                }
                while i > 0 {
                    i -= 1;
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                }
                let msg = b"\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }
            return Ok(paddr);
        }
    }

    // Debug: Log exhaustion
    unsafe {
        let msg = b"[PMM] Call #";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        print_decimal(call_num);
        let msg = b" FAILED - PMM EXHAUSTED\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        // Halt with distinctive pattern
        let msg = b"[PMM] EXHAUSTED - HALTING\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        loop {}
    }

    Err(RxStatus::ERR_NO_MEMORY)
}

/// Allocate a page from the kernel zone
///
/// This function should be used for kernel metadata allocations:
/// - Page tables
/// - Heap metadata
/// - Kernel data structures
///
/// # Returns
///
/// Physical address of the allocated page, or an error
pub fn pmm_alloc_kernel_page() -> RxResult<PAddr> {
    pmm_alloc_page(PMM_ALLOC_FLAG_KERNEL)
}

/// Allocate a page from the user zone
///
/// This function should be used for user-visible memory:
/// - VMO backing pages
/// - User data
/// - Clone destinations
///
/// # Returns
///
/// Physical address of the allocated page, or an error
pub fn pmm_alloc_user_page() -> RxResult<PAddr> {
    pmm_alloc_page(PMM_ALLOC_FLAG_USER)
}

/// Allocate multiple contiguous physical pages
///
/// # Arguments
///
/// * `count` - Number of pages to allocate
/// * `flags` - Allocation flags
/// * `align_log2` - Alignment as log2 (0 = page aligned, 12 = 4KB, etc.)
///
/// # Returns
///
/// Physical address of the allocated region, or an error
///
/// Note: For the Vec-based PMM, contiguous allocation is implemented
/// by finding sequential free pages. This is simpler than the bitmap version
/// but may be slower for large allocations.
pub fn pmm_alloc_contiguous(count: usize, flags: u32, _align_log2: u8) -> RxResult<PAddr> {
    if count == 0 {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }

    // For single pages, use the regular allocator
    if count == 1 {
        return pmm_alloc_page(flags);
    }

    // For multiple pages, try to find contiguous free pages
    let arenas = unsafe { &mut ARENAS[..NUM_ARENAS] };

    for arena in arenas {
        if flags == PMM_ALLOC_FLAG_LOW_MEM && (arena.info.flags & ARENA_FLAG_LOW_MEM) == 0 {
            continue;
        }

        // Try to find contiguous free pages
        let mut start_index = 0;
        let total_count = arena.total_count as usize;

        while start_index + count <= total_count {
            // Check if all pages in this range are free
            let mut all_free = true;
            for page_idx in start_index..(start_index + count) {
                if !arena.pages[page_idx].is_free() {
                    all_free = false;
                    start_index = page_idx + 1; // Start from next page
                    break;
                }
            }

            if all_free {
                // Mark all pages as allocated
                for page_idx in start_index..(start_index + count) {
                    arena.pages[page_idx].state = PageState::Allocated;
                    arena.pages[page_idx].ref_count = 1;
                }
                return Ok(arena.info.base + (start_index as PAddr) * PAGE_SIZE as PAddr);
            }
        }
    }

    Err(RxStatus::ERR_NO_MEMORY)
}

/// Free a physical page
///
/// # Arguments
///
/// * `paddr` - Physical address of the page to free
///
/// # Returns
///
/// `RxStatus::OK` on success, or an error code
pub fn pmm_free_page(paddr: PAddr) -> RxStatus {
    let arenas = unsafe { &mut ARENAS[..NUM_ARENAS] };

    // Find the arena containing this page
    for arena in arenas {
        if arena.address_in_arena(paddr) {
            return arena.free_page(paddr);
        }
    }

    RxStatus::ERR_INVALID_ARGS
}

/// Free multiple contiguous physical pages
///
/// # Arguments
///
/// * `paddr` - Physical address of the first page to free
/// * `count` - Number of pages to free
///
/// # Returns
///
/// `RxStatus::OK` on success, or an error code
pub fn pmm_free_contiguous(paddr: PAddr, count: usize) -> RxStatus {
    if count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }

    let arenas = unsafe { &mut ARENAS[..NUM_ARENAS] };

    // Find the arena containing this page range
    for arena in arenas {
        let end_addr = paddr + (count as PAddr) * PAGE_SIZE as PAddr;

        if arena.address_in_arena(paddr) && arena.address_in_arena(end_addr - 1) {
            // Free each page
            for i in 0..count {
                let page_paddr = paddr + (i as PAddr) * PAGE_SIZE as PAddr;
                let _ = arena.free_page(page_paddr);
            }
            return RxStatus::OK;
        }
    }

    RxStatus::ERR_INVALID_ARGS
}

/// Get the number of free pages across all arenas
pub fn pmm_count_free_pages() -> u64 {
    let arenas = unsafe { &ARENAS[..NUM_ARENAS] };
    let mut count = 0u64;

    for arena in arenas {
        count += arena.count_free_pages();
    }

    count
}

/// Reserve a range of physical pages
///
/// # Arguments
///
/// * `paddr` - Starting physical address (must be page-aligned)
/// * `count` - Number of pages to reserve
///
/// # Returns
///
/// `RxStatus::OK` on success, or an error code
pub fn pmm_reserve_pages(paddr: PAddr, count: usize) -> RxStatus {
    if count == 0 {
        return RxStatus::ERR_INVALID_ARGS;
    }

    let arenas = unsafe { &mut ARENAS[..NUM_ARENAS] };

    // Find the arena containing this page range
    for arena in arenas {
        let end_addr = paddr + (count as PAddr) * PAGE_SIZE as PAddr;

        if arena.address_in_arena(paddr) && arena.address_in_arena(end_addr - 1) {
            // Mark each page as reserved
            for i in 0..count {
                let page_paddr = paddr + (i as PAddr) * PAGE_SIZE as PAddr;
                let offset = page_paddr - arena.info.base;
                let index = (offset / PAGE_SIZE as PAddr) as usize;

                if index < arena.total_count as usize {
                    arena.pages[index].state = PageState::Reserved;
                }
            }
            return RxStatus::OK;
        }
    }

    RxStatus::ERR_INVALID_ARGS
}

/// Get the total number of pages across all arenas
pub fn pmm_count_total_pages() -> u64 {
    let arenas = unsafe { &ARENAS[..NUM_ARENAS] };
    let mut count = 0u64;

    for arena in arenas {
        count += arena.count_total_pages();
    }

    count
}

/// Get the total amount of physical memory in bytes
pub fn pmm_count_total_bytes() -> u64 {
    pmm_count_total_pages() * PAGE_SIZE as u64
}

/// Convert physical address to page structure
///
/// # Arguments
///
/// * `paddr` - Physical address
///
/// # Returns
///
/// Pointer to the page structure, or null if not found
pub fn paddr_to_page(paddr: PAddr) -> *mut Page {
    let num_arenas = unsafe { NUM_ARENAS };
    let arenas = unsafe { &ARENAS[..num_arenas] };

    for arena_idx in 0..num_arenas {
        let arena = &arenas[arena_idx];
        if arena.address_in_arena(paddr) {
            let offset = paddr - arena.info.base;
            let index = (offset / PAGE_SIZE as PAddr) as usize;
            if index < arena.total_count as usize {
                // Get raw pointer to the page
                let pages_ptr = arena.pages.as_ptr() as *mut Page;
                return unsafe { pages_ptr.add(index) };
            }
        }
    }

    core::ptr::null_mut()
}

/// Kernel physical offset for direct-mapped physical memory
///
/// Physical memory is mapped at this offset in kernel virtual address space.
/// This allows the kernel to access any physical page by adding this offset.
/// Using the standard x86_64 kernel direct map offset (same as Linux).
const KERNEL_PHYS_OFFSET: u64 = 0xffff_8000_0000_0000;

/// Maximum physical memory that is identity mapped by UEFI
///
/// UEFI typically identity maps the first 2GB of physical memory.
/// For addresses below this threshold, use identity mapping.
/// For addresses above, use the direct mapping offset.
const IDENTITY_MAP_LIMIT: u64 = 0x8000_0000; // 2GB

/// Convert physical address to virtual address (for KERNEL zone only)
///
/// For kernel zone memory (heap, page tables), uses identity mapping for low memory.
/// This is safe because UEFI has identity-mapped the low memory region.
///
/// WARNING: Do NOT use this for user zone memory! Use paddr_to_vaddr_user_zone() instead.
///
/// # Safety
///
/// This function should only be used for kernel zone memory (physical addresses
/// in the range KERNEL_ZONE_START to KERNEL_ZONE_END). For user zone memory,
/// use paddr_to_vaddr_user_zone() to avoid corrupting kernel heap.
pub fn paddr_to_vaddr(paddr: PAddr) -> VAddr {
    if paddr < IDENTITY_MAP_LIMIT {
        // Low memory: use identity mapping (UEFI has this mapped)
        paddr as VAddr
    } else {
        // High memory: use direct mapping offset
        (KERNEL_PHYS_OFFSET + paddr) as VAddr
    }
}

/// Convert physical address to virtual address (for USER zone only)
///
/// ALWAYS uses the kernel's direct mapping region (KERNEL_PHYS_OFFSET).
/// This prevents user zone memory operations from corrupting kernel heap.
///
/// # Example
///
/// ```rust
/// let user_paddr = 0x1000000; // User zone physical address
/// let user_vaddr = pmm::paddr_to_vaddr_user_zone(user_paddr); // 0xffff_8010_0000_0000
/// ```
///
/// # Safety
///
/// This function should only be used for user zone memory (VMO backing pages,
/// clone destinations, etc.). For kernel zone memory, use paddr_to_vaddr() instead.
pub fn paddr_to_vaddr_user_zone(paddr: PAddr) -> VAddr {
    // For low memory (< 2GB), use identity mapping (UEFI has this mapped)
    // For high memory, use direct mapping offset
    if paddr < IDENTITY_MAP_LIMIT {
        paddr as VAddr  // Identity mapping for low memory
    } else {
        (KERNEL_PHYS_OFFSET + paddr) as VAddr  // Direct mapping for high memory
    }
}

/// Allocate a single page (convenience wrapper)
pub fn alloc_page() -> RxResult<PAddr> {
    pmm_alloc_page(0)
}

/// Free a single page (convenience wrapper)
pub fn free_page(paddr: PAddr) {
    let _ = pmm_free_page(paddr);
}
