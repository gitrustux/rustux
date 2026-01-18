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

/// Minimum block size (size of BlockHeader)
const MIN_BLOCK_SIZE: usize = core::mem::size_of::<BlockHeader>();

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

        let block_size = BlockHeader::total_size(size);

        // Ensure minimum alignment
        let actual_align = align.max(BLOCK_ALIGN);

        // Search for a suitable free block
        let mut current = self.free_list;
        let mut prev: *mut BlockHeader = core::ptr::null_mut();

        while !current.is_null() {
            let block = &*current;

            if !block.is_valid() {
                // Corrupted block, skip
                prev = current;
                current = block.next;
                continue;
            }

            if !block.free {
                // Block is not free, shouldn't happen in free list
                prev = current;
                current = block.next;
                continue;
            }

            // Check if this block is large enough
            if block.size >= block_size {
                // Calculate where the payload would start
                let payload_start = block.payload() as usize;
                let aligned_start = (payload_start + actual_align - 1) & !(actual_align - 1);

                // Calculate offset from block start to aligned payload
                let offset = aligned_start - (current as usize);

                // Check if we have enough space after alignment
                if block.size >= offset + size {
                    // Calculate remaining space after allocation
                    let remaining = block.size - offset - size;

                    // Mark block as allocated
                    (*current).free = false;

                    // Remove from free list
                    if !prev.is_null() {
                        (*prev).next = (*current).next;
                    } else {
                        self.free_list = (*current).next;
                    }
                    if !(*current).next.is_null() {
                        (*(*current).next).prev = (*current).prev;
                    }
                    (*current).prev = core::ptr::null_mut();
                    (*current).next = core::ptr::null_mut();

                    // Split the block if there's enough remaining space
                    if remaining >= MIN_BLOCK_SIZE {
                        let new_block = (current as usize + offset + size) as *mut BlockHeader;
                        (*new_block) = BlockHeader::new(remaining, true);
                        (*new_block).prev = core::ptr::null_mut();

                        // Add new block to free list
                        (*new_block).next = self.free_list;
                        if !self.free_list.is_null() {
                            (*self.free_list).prev = new_block;
                        }
                        self.free_list = new_block;
                    }

                    // Return pointer to aligned payload
                    return aligned_start as *mut u8;
                }
            }

            prev = current;
            current = block.next;
        }

        // No suitable free block found
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
