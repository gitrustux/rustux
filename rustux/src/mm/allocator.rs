// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Kernel Heap Allocator
//!
//! This module provides a linked list allocator for the kernel heap.
//! It supports allocation, deallocation, and memory reuse.
//!
//! # Design
//!
//! The allocator uses a simple linked list approach where free blocks
//! are tracked in a free list. When allocating, it searches for a
//! suitable free block, potentially splitting it if needed. When freeing,
//! it merges adjacent free blocks to reduce fragmentation.
//!
//! # Usage
//!
//! ```rust
//! use rustux::mm::allocator::*;
//!
//! // Initialize the heap with a memory region
//! unsafe { init(heap_start, heap_size); }
//!
//! // Allocate memory
//! let ptr = allocate(size, align);
//!
//! // Free memory
//! deallocate(ptr, size, align);
//! ```

use crate::arch::amd64::mm::page_tables::PAGE_SIZE;

// Align helper function (local to this module)
fn align_page_up(addr: usize) -> usize {
    const PAGE_MASK: usize = PAGE_SIZE - 1;
    (addr + PAGE_MASK) & !PAGE_MASK
}

/// Default heap size (16 MB)
pub const DEFAULT_HEAP_SIZE: usize = 16 * 1024 * 1024;

/// Minimum block size - increased to reduce fragmentation
/// Blocks smaller than this won't be split off during allocation
const MIN_BLOCK_SIZE: usize = 1024;

/// Align block size to pointer alignment
const BLOCK_ALIGN: usize = core::mem::align_of::<BlockHeader>();

/// Heap block header for free list
#[repr(C)]
#[derive(Debug)]
struct BlockHeader {
    /// Size of this block (including header)
    size: usize,

    /// Whether this block is free
    free: bool,

    /// Magic value for corruption detection
    magic: u64,

    /// Previous block in the list (raw pointer for easier manipulation)
    prev: *mut BlockHeader,

    /// Next block in the list (raw pointer for easier manipulation)
    next: *mut BlockHeader,
}

/// Magic value to detect heap corruption
const BLOCK_MAGIC: u64 = 0xDEADBEEFCAFEBABE;

impl BlockHeader {
    /// Create a new block header
    const fn new(size: usize, free: bool) -> Self {
        Self {
            size,
            free,
            magic: BLOCK_MAGIC,
            prev: core::ptr::null_mut(),
            next: core::ptr::null_mut(),
        }
    }

    /// Get the end of this block (first byte after the block)
    fn end(&self) -> *mut u8 {
        (self as *const BlockHeader as usize + self.size) as *mut u8
    }

    /// Get the payload (data area) of this block
    fn payload(&self) -> *mut u8 {
        (self as *const BlockHeader as usize + core::mem::size_of::<BlockHeader>()) as *mut u8
    }

    /// Check if this block's magic is valid
    fn is_valid(&self) -> bool {
        self.magic == BLOCK_MAGIC
    }

    /// Calculate the total size needed for an allocation (including header)
    fn total_size(alloc_size: usize) -> usize {
        let size = alloc_size + core::mem::size_of::<BlockHeader>();
        size.max(MIN_BLOCK_SIZE)
    }
}

/// Linked list allocator state
#[derive(Debug)]
pub struct LinkedListAllocator {
    /// First free block in the heap
    free_list: *mut BlockHeader,

    /// Start of the heap region
    heap_start: usize,

    /// Total size of the heap
    heap_size: usize,

    /// Whether the allocator has been initialized
    initialized: bool,
}

impl LinkedListAllocator {
    /// Create a new unlinked allocator
    pub const fn new() -> Self {
        Self {
            free_list: core::ptr::null_mut(),
            heap_start: 0,
            heap_size: 0,
            initialized: false,
        }
    }

    /// Initialize the allocator with a memory region
    ///
    /// # Arguments
    ///
    /// * `heap_start` - Starting address of the heap region
    /// * `heap_size` - Size of the heap region in bytes
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The memory region is valid and accessible
    /// - The memory region is large enough for at least one block
    /// - The memory region is properly aligned
    pub unsafe fn init(&mut self, heap_start: usize, heap_size: usize) {
        self.heap_start = heap_start;
        self.heap_size = heap_size;
        self.initialized = true;

        // Print heap init telemetry once
        let msg = b"[HEAP] init base=0x";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = heap_start;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
            n >>= 4;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg = b" size=";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let size_mb = heap_size / (1024 * 1024);
        let mut n = size_mb;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg = b"MB\n";
        for &byte in msg {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        // Initialize the heap as a single free block
        let block = heap_start as *mut BlockHeader;
        (*block) = BlockHeader::new(heap_size, true);
        (*block).prev = core::ptr::null_mut();
        (*block).next = core::ptr::null_mut();

        self.free_list = block;
    }

    /// Allocate memory from the heap
    ///
    /// # Arguments
    ///
    /// * `size` - Size of the allocation in bytes
    /// * `align` - Required alignment in bytes
    ///
    /// # Returns
    ///
    /// Pointer to the allocated memory, or null if allocation failed
    ///
    /// # Safety
    ///
    /// This function modifies the heap's internal state.
    pub unsafe fn allocate(&mut self, size: usize, align: usize) -> *mut u8 {
        if !self.initialized || size == 0 {
            return core::ptr::null_mut();
        }

        // Log allocation request for first 30 allocations
        static mut ALLOC_COUNT: u32 = 0;
        ALLOC_COUNT += 1;
        if ALLOC_COUNT <= 30 {
            let msg = b"[HEAP] alloc request size=";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = size;
            let mut buf = [0u8; 16];
            let mut i = 0;
            loop {
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
                i += 1;
                if n == 0 { break; }
            }
            while i > 0 {
                i -= 1;
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
            }

            let msg = b" align=";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
            let mut n = align;
            let mut buf = [0u8; 16];
            let mut i = 0;
            loop {
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
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

        let block_size = BlockHeader::total_size(size);

        // Ensure minimum alignment
        let actual_align = align.max(BLOCK_ALIGN);

        // Search for a suitable free block
        let mut current = self.free_list;
        let mut prev: *mut BlockHeader = core::ptr::null_mut();

        // Debug: entering while loop
        static mut LOOP_COUNT: u32 = 0;
        let my_loop = LOOP_COUNT;
        LOOP_COUNT += 1;

        if my_loop < 20 {
            let msg = b"[HEAP] entering while loop\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        // Checkpoint: track iterations
        let mut iteration_count = 0u32;

        while !current.is_null() {
            iteration_count += 1;

            // Print checkpoint every 100 iterations
            if iteration_count % 100 == 0 && my_loop < 20 {
                // Simple checkpoint: print '.'
                let msg = b".";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            let block = &*current;

            if my_loop < 20 {
                let msg = b"[HEAP] loop iteration\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                // Print block size for debugging
                let msg = b"[HEAP] block size=";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                let mut n = block.size;
                let mut buf = [0u8; 16];
                let mut i = 0;
                loop {
                    buf[i] = b'0' + (n % 10) as u8;
                    n /= 10;
                    i += 1;
                    if n == 0 { break; }
                }
                while i > 0 {
                    i -= 1;
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                }
                let msg = b" free=";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                let digit = if block.free { b'1' } else { b'0' };
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                let msg = b"\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            if !block.is_valid() {
                // Corrupted block, skip
                if my_loop < 20 {
                    let msg = b"[HEAP] block invalid, skipping\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
                prev = current;
                current = block.next;
                continue;
            }

            if !block.free {
                // Block is not free, shouldn't happen in free list
                if my_loop < 20 {
                    let msg = b"[HEAP] block not free, skipping\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
                prev = current;
                current = block.next;
                continue;
            }

            // Check if this block is large enough
            if block.size >= block_size {
                if my_loop < 20 {
                    let msg = b"[HEAP] block large enough, using it\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }
                // Calculate where the payload would start
                let payload_start = block.payload() as usize;
                let aligned_start = (payload_start + actual_align - 1) & !(actual_align - 1);

                // Calculate offset from block start to aligned payload
                let offset = aligned_start - (current as usize);

                // Check if we have enough space after alignment
                if block.size >= offset + size {
                    // MARKER: Confirm we reached this point
                    if my_loop < 20 {
                        let msg = b"[HEAP] MARKER: Reached remaining calc\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Calculate remaining space after allocation
                    let remaining = block.size - offset - size;

                    // Debug: print remaining value
                    if my_loop < 20 {
                        let msg = b"[HEAP] remaining=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = remaining;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = b'0' + (n % 10) as u8;
                            n /= 10;
                            i += 1;
                            if n == 0 { break; }
                        }
                        while i > 0 {
                            i -= 1;
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                        }
                        let msg = b" MIN_BLOCK_SIZE=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = MIN_BLOCK_SIZE;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = b'0' + (n % 10) as u8;
                            n /= 10;
                            i += 1;
                            if n == 0 { break; }
                        }
                        while i > 0 {
                            i -= 1;
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                        }
                        let msg = b" will_split=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let digit = if remaining >= MIN_BLOCK_SIZE { b'1' } else { b'0' };
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                        let msg = b"\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Mark block as allocated
                    (*current).free = false;

                    // CRITICAL: Clear the next/prev pointers to prevent stale pointers
                    // from corrupting the heap when this block is reallocated
                    (*current).next = core::ptr::null_mut();
                    (*current).prev = core::ptr::null_mut();

                    // Debug: Print before updating free_list
                    if my_loop < 20 {
                        let msg = b"[HEAP] CRITICAL: About to update free_list\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        // Print current block address
                        let msg = b"[HEAP] current=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = current as usize;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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

                    // Debug: Print current.next before dereferencing
                    if my_loop < 20 {
                        let msg = b"[HEAP] current.next=";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = (*current).next as usize;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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

                    // Remove from free list
                    // DEBUG: Track what we're about to write
                    if my_loop < 20 && !prev.is_null() {
                        let msg = b"[HEAP] Will write to prev.next at 0x";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = &(*prev).next as *const _ as usize;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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

                    if !prev.is_null() {
                        if my_loop < 20 {
                            let msg = b"[HEAP] Writing to prev.next at 0x";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = &(*prev).next as *const _ as usize;
                            let mut buf = [0u8; 16];
                            let mut i = 0;
                            loop {
                                buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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
                        (*prev).next = (*current).next;
                    } else {
                        self.free_list = (*current).next;
                    }
                    if !(*current).next.is_null() {
                        if my_loop < 20 {
                            let msg = b"[HEAP] Writing to current.next.prev at 0x";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = &(*(*current).next).prev as *const _ as usize;
                            let mut buf = [0u8; 16];
                            let mut i = 0;
                            loop {
                                buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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
                        (*(*current).next).prev = (*current).prev;
                    }
                    (*current).prev = core::ptr::null_mut();
                    (*current).next = core::ptr::null_mut();

                    // Debug: Print after updating free_list
                    if my_loop < 20 {
                        let msg = b"[HEAP] CRITICAL: free_list updated\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Split the block if there's enough remaining space for a BlockHeader
                    // Lower threshold to ensure free_list never becomes empty
                    if remaining >= MIN_BLOCK_SIZE {
                        let header_size = core::mem::size_of::<BlockHeader>();

                        // Calculate where the new free block should start (after the allocated portion)
                        // The allocated portion is: header (40 bytes) + offset (alignment padding) + size (requested)
                        let alloc_end = current as usize + offset + size;

                        // Round UP to nearest header_size (40 bytes) boundary for the new block
                        let aligned_new_block = (alloc_end + header_size - 1) & !(header_size - 1);

                        // Calculate the size of the new free block (from aligned_new_block to end of original block)
                        let new_block_size = (current as usize + block.size) - aligned_new_block;

                        // CRITICAL FIX: Update the original block's size to only include the allocated portion
                        // This prevents the original block from being found again in future allocations
                        (*current).size = offset + size;

                        // Debug: check if splitting will happen
                        if my_loop < 20 {
                            let msg = b"[HEAP] SPLIT: remaining=";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = remaining;
                            let mut buf = [0u8; 16];
                            let mut i = 0;
                            loop {
                                buf[i] = b'0' + (n % 10) as u8;
                                n /= 10;
                                i += 1;
                                if n == 0 { break; }
                            }
                            while i > 0 {
                                i -= 1;
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                            }
                            let msg = b" new_block_size=";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = new_block_size;
                            let mut buf = [0u8; 16];
                            let mut i = 0;
                            loop {
                                buf[i] = b'0' + (n % 10) as u8;
                                n /= 10;
                                i += 1;
                                if n == 0 { break; }
                            }
                            while i > 0 {
                                i -= 1;
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                            }
                            let msg = b" MIN_BLOCK_SIZE=";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = MIN_BLOCK_SIZE;
                            let mut buf = [0u8; 16];
                            let mut i = 0;
                            loop {
                                buf[i] = b'0' + (n % 10) as u8;
                                n /= 10;
                                i += 1;
                                if n == 0 { break; }
                            }
                            while i > 0 {
                                i -= 1;
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
                            }
                            let msg = b" will_split=";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let digit = if new_block_size >= MIN_BLOCK_SIZE { b'1' } else { b'0' };
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                            let msg = b"\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        // Only split if we have enough space after alignment
                        if new_block_size >= MIN_BLOCK_SIZE {
                            // CRITICAL FIX: Update the original block's size to only include the allocated portion
                            // This prevents the original block from being found again in future allocations
                            (*current).size = offset + size;

                            let new_block = aligned_new_block as *mut BlockHeader;
                            (*new_block) = BlockHeader::new(new_block_size, true);
                            (*new_block).prev = core::ptr::null_mut();

                            // Add new block to free list
                            (*new_block).next = self.free_list;
                            if !self.free_list.is_null() {
                                (*self.free_list).prev = new_block;
                            }
                            self.free_list = new_block;

                            if my_loop < 20 {
                                let msg = b"[HEAP] SPLIT: Created new block at 0x";
                                for &byte in msg {
                                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                                }
                                let mut n = aligned_new_block;
                                let mut buf = [0u8; 16];
                                let mut i = 0;
                                loop {
                                    buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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
                        }
                    }

                    // Return pointer to aligned payload
                    if my_loop < 20 {
                        let msg = b"[HEAP] allocation succeeded, returning: 0x";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = aligned_start;
                        let mut buf = [0u8; 16];
                        let mut i = 0;
                        loop {
                            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
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

                    return aligned_start as *mut u8;
                }
            }

            if my_loop < 20 {
                let msg = b"[HEAP] block too small, moving to next\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            prev = current;
            current = block.next;
        }

        // No suitable free block found
        if my_loop < 20 {
            let msg = b"[HEAP] no suitable block found, allocation failed\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }
        core::ptr::null_mut()
    }

    /// Free memory back to the heap
    ///
    /// # Arguments
    ///
    /// * `ptr` - Pointer to the memory to free
    /// * `_size` - Size of the allocation (for validation, may be 0)
    /// * `_align` - Alignment of the allocation (may be 0)
    ///
    /// # Safety
    ///
    /// This function modifies the heap's internal state.
    /// The pointer must have been returned by a previous call to allocate.
    pub unsafe fn deallocate(&mut self, ptr: *mut u8, _size: usize, _align: usize) {
        if !self.initialized || ptr.is_null() {
            return;
        }

        // Get the block header (it's before the payload)
        // Find the block by searching backwards from the payload
        let mut block = self.heap_start as *mut BlockHeader;

        while (block as usize) < (self.heap_start + self.heap_size) {
            if !(*block).is_valid() {
                break; // Corruption detected
            }

            let payload_start = (*block).payload();
            if payload_start == ptr || (payload_start < ptr && (ptr as usize) < ((*block).end() as usize)) {
                // Found the block
                (*block).free = true;

                // Try to merge with next block if it's free
                let next = (*block).next;
                if !next.is_null() && (*next).is_valid() && (*next).free {
                    // Merge with next block
                    (*block).size += (*next).size;
                    (*block).next = (*next).next;
                    let next_next = (*next).next;
                    if !next_next.is_null() {
                        (*next_next).prev = block;
                    }
                }

                // Try to merge with previous block if it's free
                // We need to find the previous block by scanning
                let mut prev_block = self.heap_start as *mut BlockHeader;
                while (prev_block as usize) < (block as usize) {
                    if !(*prev_block).is_valid() {
                        break;
                    }

                    let prev_end = (*prev_block).end() as usize;
                    if prev_end == (block as usize) && (*prev_block).free {
                        // Found adjacent previous free block, merge
                        (*prev_block).size += (*block).size;
                        (*prev_block).next = (*block).next;
                        let next = (*block).next;
                        if !next.is_null() {
                            (*next).prev = prev_block;
                        }
                        block = prev_block;
                        break;
                    }

                    prev_block = (*prev_block).end() as *mut BlockHeader;
                }

                // Add to free list
                (*block).next = self.free_list;
                (*block).prev = core::ptr::null_mut();
                if !self.free_list.is_null() {
                    (*self.free_list).prev = block;
                }
                self.free_list = block;

                return;
            }

            block = (*block).end() as *mut BlockHeader;
        }
    }

    /// Get heap usage statistics
    ///
    /// # Returns
    ///
    /// Number of bytes currently allocated
    pub fn usage(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut used = 0usize;
        unsafe {
            let mut block = self.heap_start as *mut BlockHeader;

            while (block as usize) < (self.heap_start + self.heap_size) {
                if !(*block).is_valid() {
                    break;
                }

                if !(*block).free {
                    used += (*block).size;
                }

                block = (*block).end() as *mut BlockHeader;
            }
        }

        used
    }

    /// Get total heap size
    ///
    /// # Returns
    ///
    /// Total size of the heap in bytes
    pub fn size(&self) -> usize {
        self.heap_size
    }

    /// Get available (free) heap size
    ///
    /// # Returns
    ///
    /// Number of bytes currently available for allocation
    pub fn available(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut free_bytes = 0usize;
        unsafe {
            let mut current = self.free_list;

            while !current.is_null() {
                let block = &*current;
                if block.is_valid() && block.free {
                    free_bytes += block.size;
                }
                current = block.next;
            }
        }

        free_bytes
    }

    /// Count the number of free blocks
    pub fn free_block_count(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut count = 0usize;
        unsafe {
            let mut current = self.free_list;

            while !current.is_null() {
                let block = &*current;
                if block.is_valid() && block.free {
                    count += 1;
                }
                current = block.next;
            }
        }

        count
    }

    /// Print heap summary
    pub unsafe fn print_summary(&self) {
        if !self.initialized {
            return;
        }

        let used = self.usage();
        let avail = self.available();
        let free_blocks = self.free_block_count();

        // Print in format: [HEAP] base=0x300000 size=16MB used=X avail=Y free_blocks=Z
        let msg_start = b"[HEAP] base=0x";
        for &byte in msg_start {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = self.heap_start;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = if (n & 0xF) < 10 { b'0' + (n & 0xF) as u8 } else { b'a' + (n & 0xF) as u8 - 10 };
            n >>= 4;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg_size = b" size=";
        for &byte in msg_size {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let size_mb = self.heap_size / (1024 * 1024);
        let mut n = size_mb;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }
        let msg_mb = b"MB";
        for &byte in msg_mb {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }

        let msg_used = b" used=";
        for &byte in msg_used {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = used;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg_avail = b" avail=";
        for &byte in msg_avail {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = avail;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg_blocks = b" free_blocks=";
        for &byte in msg_blocks {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
        let mut n = free_blocks;
        let mut buf = [0u8; 16];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
            if n == 0 { break; }
        }
        while i > 0 {
            i -= 1;
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") buf[i], options(nomem, nostack));
        }

        let msg_newline = b"\n";
        for &byte in msg_newline {
            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
        }
    }
}

/// Global allocator instance
static mut ALLOCATOR: LinkedListAllocator = LinkedListAllocator::new();

/// Initialize the heap allocator
///
/// # Arguments
///
/// * `heap_start` - Starting address of the heap region
/// * `heap_size` - Size of the heap region in bytes
///
/// # Safety
///
/// The caller must ensure that:
/// - The memory region is valid and accessible
/// - The memory region is not used for any other purpose
/// - This function is called only once during initialization
pub unsafe fn init(heap_start: usize, heap_size: usize) {
    ALLOCATOR.init(heap_start, heap_size);
}

/// Initialize the heap allocator with a page-aligned heap
///
/// This is a convenience function that creates a heap of the specified
/// size, starting at the given address. The heap will be page-aligned.
///
/// # Arguments
///
/// * `start_addr` - Starting address for the heap (will be page-aligned)
/// * `size` - Size of the heap in bytes (will be rounded up to page-aligned)
///
/// # Returns
///
/// The aligned starting address of the heap
///
/// # Safety
///
/// The caller must ensure that:
/// - The memory region is valid and accessible
/// - The memory region is large enough
/// - This function is called only once during initialization
pub unsafe fn init_aligned(start_addr: usize, size: usize) -> usize {
    let aligned_start = align_page_up(start_addr);
    let aligned_size = align_page_up(size);
    init(aligned_start, aligned_size);
    aligned_start
}

/// Allocate memory from the heap
///
/// # Arguments
///
/// * `size` - Size of the allocation in bytes
/// * `align` - Required alignment in bytes
///
/// # Returns
///
/// Pointer to the allocated memory, or null if allocation failed
///
/// # Safety
///
/// This function modifies the heap's internal state.
pub unsafe fn allocate(size: usize, align: usize) -> *mut u8 {
    ALLOCATOR.allocate(size, align)
}

/// Free memory back to the heap
///
/// # Arguments
///
/// * `ptr` - Pointer to the memory to free
/// * `size` - Size of the original allocation
/// * `align` - Alignment of the original allocation
///
/// # Safety
///
/// This function modifies the heap's internal state.
pub unsafe fn deallocate(ptr: *mut u8, size: usize, align: usize) {
    ALLOCATOR.deallocate(ptr, size, align)
}

/// Get heap usage statistics
///
/// # Returns
///
/// Number of bytes currently allocated
pub fn heap_usage() -> usize {
    unsafe { ALLOCATOR.usage() }
}

/// Get total heap size
///
/// # Returns
///
/// Total size of the heap in bytes
pub fn heap_size() -> usize {
    unsafe { ALLOCATOR.size() }
}

/// Get available heap size
///
/// # Returns
///
/// Number of bytes currently available for allocation
pub fn heap_available() -> usize {
    unsafe { ALLOCATOR.available() }
}

/// Print heap summary for debugging
pub fn heap_print_summary() {
    unsafe { ALLOCATOR.print_summary() }
}

// ============================================================================
// GlobalAlloc Implementation
// ============================================================================

use alloc::alloc::{GlobalAlloc, Layout};

// SAFETY: The allocator is only used in a single-threaded early kernel environment
// The raw pointers inside are protected by the fact that there's no concurrent access
unsafe impl Sync for LinkedListAllocator {}

unsafe impl GlobalAlloc for LinkedListAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Use a mutable reference to self for the allocation
        // Note: This is a workaround - GlobalAlloc takes &self but we need &mut self
        // In a single-threaded early kernel environment, this is safe
        let allocator = &ALLOCATOR as *const LinkedListAllocator as *mut LinkedListAllocator;
        (*allocator).allocate(layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let allocator = &ALLOCATOR as *const LinkedListAllocator as *mut LinkedListAllocator;
        (*allocator).deallocate(ptr, layout.size(), layout.align());
    }
}

/// Global heap allocator instance
///
/// This is exported as the global allocator for the Rust standard library.
#[global_allocator]
static HEAP_ALLOCATOR: LinkedListAllocator = LinkedListAllocator::new();

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_HEAP_SIZE: usize = 64 * 1024; // 64 KB for testing

    #[test]
    fn test_allocator_init() {
        unsafe {
            let heap: [u8; TEST_HEAP_SIZE] = [0; TEST_HEAP_SIZE];
            init(heap.as_ptr() as usize, TEST_HEAP_SIZE);
            assert_eq!(heap_size(), TEST_HEAP_SIZE);
            assert_eq!(heap_usage(), 0);
            assert!(heap_available() > 0);
        }
    }

    #[test]
    fn test_allocator_allocate() {
        unsafe {
            let heap: [u8; TEST_HEAP_SIZE] = [0; TEST_HEAP_SIZE];
            init(heap.as_ptr() as usize, TEST_HEAP_SIZE);

            let ptr = allocate(1024, 8);
            assert!(!ptr.is_null());
            assert!(heap_usage() >= 1024);

            deallocate(ptr, 1024, 8);
            // After deallocation, usage should be back to 0
            // (but may not be exactly 0 due to block splitting)
        }
    }
}
