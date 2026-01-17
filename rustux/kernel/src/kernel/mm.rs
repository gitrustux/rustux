// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style

//! Memory management (stub for now)

/// Page mask
pub const PAGE_SIZE: usize = 4096;

/// Bump allocator stub
pub struct BumpAllocator {
    start: u64,
    end: u64,
    current: u64,
}

impl BumpAllocator {
    pub fn new(start: u64, size: u64) -> Option<Self> {
        Some(Self {
            start,
            end: start + size,
            current: start,
        })
    }

    pub fn allocate(&mut self, _size: u64) -> Option<u64> {
        if self.current >= self.end {
            None
        } else {
            let addr = self.current;
            self.current += _size;
            Some(addr)
        }
    }
}
