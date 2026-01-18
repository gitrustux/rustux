// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! x86-64 Cache Operations
//!
//! This module provides cache manipulation functions for x86-64 processors.
//!
//! # Cache Operations
//!
//! - CLFLUSH: Flush a cache line
//! - CLFLUSHOPT: Flush a cache line with optimization
//! - WBINVD: Write-back and invalidate all caches
//! - MFENCE: Memory fence for cache coherency

use crate::arch::amd64::registers;

/// Get data cache line size
///
/// # Returns
///
/// The size of the data cache line in bytes
pub fn arch_dcache_line_size() -> usize {
    unsafe { x86_get_clflush_line_size() as usize }
}

/// Get instruction cache line size
///
/// # Returns
///
/// The size of the instruction cache line in bytes
pub fn arch_icache_line_size() -> usize {
    unsafe { x86_get_clflush_line_size() as usize }
}

/// Synchronize the cache for the given range
///
/// Uses cpuid as a serializing instruction to ensure visibility
/// of instruction stream modifications (self/cross-modifying code).
///
/// # Arguments
///
/// * `_start` - Starting virtual address
/// * `_len` - Length of the range in bytes
pub fn arch_sync_cache_range(_start: usize, _len: usize) {
    // Invoke cpuid to act as a serializing instruction
    // This ensures we see modifications to the instruction stream
    // See Intel Volume 3, 8.1.3 "Handling Self- and Cross-Modifying Code"
    #[cfg(target_arch = "x86_64")]
    {
        let result = core::arch::x86_64::__cpuid(0);
        let _ = result.edx; // Prevent unused warning
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // For other architectures, use a compiler barrier
        core::hint::black_box(());
    }
}

/// Invalidate the cache for the given range
///
/// # Arguments
///
/// * `_start` - Starting virtual address
/// * `_len` - Length of the range in bytes
pub fn arch_invalidate_cache_range(_start: usize, _len: usize) {
    // No-op on x86 for instruction cache invalidation
}

/// Clean the cache for the given range
///
/// # Arguments
///
/// * `start` - Starting virtual address
/// * `len` - Length of the range in bytes
pub fn arch_clean_cache_range(start: usize, len: usize) {
    // TODO: consider wiring up clwb if present
    arch_clean_invalidate_cache_range(start, len);
}

/// Clean and invalidate the cache for the given range
///
/// # Arguments
///
/// * `start` - Starting virtual address
/// * `len` - Length of the range in bytes
pub fn arch_clean_invalidate_cache_range(start: usize, len: usize) {
    // Check if CLFLUSH is available (we assume it is for modern x86-64)
    let clsize = unsafe { x86_get_clflush_line_size() } as usize;
    let end = start + len;
    let mut ptr = start & !(clsize - 1); // Align down to cache line

    // For now, assume CLFLUSH is available (it is on all modern x86-64)
    // TODO: Check CPUID for CLFLUSH support
    while ptr < end {
        unsafe {
            core::arch::asm!("clflush [{ptr}]", ptr = in(reg) ptr);
        }
        ptr += clsize;
    }

    // Memory fence to ensure cache operations complete
    unsafe {
        registers::x86_mfence();
    }
}

/// Get the cache line size for CLFLUSH
///
/// This reads CPUID to determine the cache line size.
///
/// # Returns
///
/// The cache line size in bytes
///
/// # Safety
///
/// This function uses inline assembly to read CPUID.
unsafe fn x86_get_clflush_line_size() -> u32 {
    // Use cpuid leaf 1 to get clflush line size
    let result = core::arch::x86_64::__cpuid(1);

    // Bits 15-08: CLFLUSH line size (value * 8 = cache line size in bytes)
    let clflush_size = ((result.ebx >> 8) & 0xFF) as u32;

    // Multiply by 8 to get the actual size
    clflush_size * 8
}

/// Write-back and invalidate all caches
///
/// # Safety
///
/// This is a privileged instruction that should only be used
/// in kernel mode
#[inline]
pub unsafe fn wbinvd() {
    core::arch::asm!("wbinvd", options(nomem, nostack));
}
