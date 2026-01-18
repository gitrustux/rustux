// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Memory Management
//!
//! This module provides memory management services for the kernel,
//! including physical memory allocation and heap allocation.
//!
//! # Modules
//!
//! - [`pmm`] - Physical Memory Manager for allocating physical pages
//! - [`allocator`] - Heap allocator for dynamic memory allocation
//!
//! # Usage
//!
//! ```rust
//! use rustux::mm::*;
//!
//! // Allocate a physical page
//! let page = pmm::pmm_alloc_page(0)?;
//!
//! // Allocate from heap (requires allocator initialization)
//! let ptr = unsafe { allocator::allocate(1024, 8) };
//! ```
//!
//! # Initialization
//!
//! The PMM must be initialized with memory arenas before use:
//!
//! ```rust
//! use rustux::mm::pmm::*;
//!
//! unsafe {
//!     // Set up boot allocator first
//!     pmm::set_boot_allocator(boot_alloc_fn);
//!
//!     // Add memory arenas
//!     pmm::pmm_init_early(low_base, low_size, Some(high_base), Some(high_size));
//! }
//! ```
//!
//! The heap allocator must be initialized with a memory region:
//!
//! ```rust
//! use rustux::mm::allocator::*;
//!
//! unsafe {
//!     allocator::init(heap_start_addr, heap_size);
//! }
//! ```

pub mod pmm;
pub mod allocator;

// Re-export PAGE_SIZE explicitly from page_tables to avoid ambiguity
pub use crate::arch::amd64::mm::page_tables::PAGE_SIZE;

// Re-export commonly used types and functions from pmm
pub use pmm::{
    // Page constants
    PAGE_SIZE_SHIFT,
    PAGE_MASK,
    is_page_aligned,
    align_page_down,
    align_page_up,
    bytes_to_pages,
    pages_to_bytes,
    // Page state and arena types
    PageState,
    ArenaInfo,
    Page,
    // Arena flags
    ARENA_FLAG_LOW_MEM,
    ARENA_FLAG_HIGH_MEM,
    PMM_ALLOC_FLAG_ANY,
    PMM_ALLOC_FLAG_LOW_MEM,
    // PMM functions
    set_boot_allocator,
    pmm_add_arena,
    pmm_alloc_page,
    pmm_alloc_contiguous,
    pmm_free_page,
    pmm_free_contiguous,
    pmm_count_free_pages,
    pmm_count_total_pages,
    pmm_count_total_bytes,
    paddr_to_page,
    pmm_init_early,
    // Convenience wrappers
    alloc_page,
    free_page,
    paddr_to_vaddr,
};

// Re-export commonly used types and functions from allocator
pub use allocator::{
    init as heap_init,
    init_aligned as heap_init_aligned,
    allocate as heap_allocate,
    deallocate as heap_deallocate,
    heap_usage,
    heap_size,
    heap_available,
    DEFAULT_HEAP_SIZE,
};

/// Memory management error type
pub type Result<T> = crate::arch::amd64::mm::RxResult<T>;

/// Memory management status type
pub type Status = crate::arch::amd64::mm::RxStatus;
