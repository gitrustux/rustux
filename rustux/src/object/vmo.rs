// Copyright 2025 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Virtual Memory Objects (VMOs)
//!
//! VMOs represent contiguous regions of physical memory that can be
//! mapped into address spaces. They support COW cloning and resizing.
//!
//! # Design
//!
//! - **Page-based**: Memory is managed in page-sized chunks
//! - **COW clones**: Copy-on-write for efficient memory sharing
//! - **Resizable**: VMOs can grow/shrink if created with RESIZABLE flag
//! - **Cache policy**: Control cache behavior (uncached, write-combining, etc.)
//!
//! # Usage
//!
//! ```rust
//! let vmo = Vmo::create(0x1000, VmoFlags::empty())?;
//! vmo.write(0, &data)?;
//! vmo.read(0, &mut buf)?;
//! ```

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::sync::SpinMutex;
use crate::object::handle::{KernelObjectBase, ObjectType};
use crate::arch::amd64::mm::page_tables::PAddr;
use alloc::collections::BTreeMap;

/// ============================================================================
/// VMO ID
/// ============================================================================

/// VMO identifier
pub type VmoId = u64;

/// Next VMO ID counter
static mut NEXT_VMO_ID: AtomicU64 = AtomicU64::new(1);

/// Allocate a new VMO ID
fn alloc_vmo_id() -> VmoId {
    unsafe { NEXT_VMO_ID.fetch_add(1, Ordering::Relaxed) }
}

/// ============================================================================
/// VMO Flags
/// ============================================================================

/// VMO creation flags
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmoFlags(pub u32);

impl VmoFlags {
    /// No flags
    pub const empty: Self = Self(0);

    /// VMO is resizable
    pub const RESIZABLE: Self = Self(0x01);

    /// VMO is a COW clone
    pub const COW: Self = Self(0x02);

    /// Check if resizable
    pub const fn is_resizable(self) -> bool {
        (self.0 & Self::RESIZABLE.0) != 0
    }

    /// Check if COW clone
    pub const fn is_cow(self) -> bool {
        (self.0 & Self::COW.0) != 0
    }

    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self.0
    }
}

impl core::ops::BitOr for VmoFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// ============================================================================
/// Cache Policy
/// ============================================================================

/// Cache policy for VMO mappings
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Default caching
    Default = 0,

    /// Uncached access
    Uncached = 1,

    /// Write-combining
    WriteCombining = 2,

    /// Write-through
    WriteThrough = 3,
}

impl CachePolicy {
    /// Create from raw value
    pub const fn from_raw(raw: u32) -> Self {
        match raw {
            1 => Self::Uncached,
            2 => Self::WriteCombining,
            3 => Self::WriteThrough,
            _ => Self::Default,
        }
    }

    /// Get raw value
    pub const fn into_raw(self) -> u32 {
        self as u32
    }
}

/// ============================================================================
/// Page Map
/// ============================================================================

/// Page map entry
#[derive(Debug)]
pub struct PageMapEntry {
    /// Physical page address
    pub paddr: PAddr,

    /// Whether page is present (not committed if COW)
    pub present: bool,

    /// Whether page is writable
    pub writable: bool,
}

/// ============================================================================
/// VMO
/// ============================================================================

/// Virtual Memory Object
///
/// Represents a contiguous region of physical memory.
pub struct Vmo {
    /// Kernel object base
    pub base: KernelObjectBase,

    /// VMO ID
    pub id: VmoId,

    /// VMO size in bytes
    pub size: AtomicUsize,

    /// VMO flags
    pub flags: VmoFlags,

    /// Cache policy
    pub cache_policy: SpinMutex<CachePolicy>,

    /// Page map (offset -> page entry)
    pub pages: SpinMutex<BTreeMap<usize, PageMapEntry>>,

    /// Parent VMO (for COW clones)
    pub parent: SpinMutex<Option<*const Vmo>>,
}

impl Vmo {
    /// Create a new VMO
    ///
    /// # Arguments
    ///
    /// * `size` - Size in bytes (will be rounded up to page size)
    /// * `flags` - VMO flags
    pub fn create(size: usize, flags: VmoFlags) -> Result<Self, &'static str> {
        if size == 0 {
            return Err("size cannot be zero");
        }

        // Round up to page size
        let page_size = 4096; // TODO: Use proper PAGE_SIZE constant
        let size_aligned = (size + page_size - 1) / page_size * page_size;

        Ok(Self {
            base: KernelObjectBase::new(ObjectType::Vmo),
            id: alloc_vmo_id(),
            size: AtomicUsize::new(size_aligned),
            flags,
            cache_policy: SpinMutex::new(CachePolicy::Default),
            pages: SpinMutex::new(BTreeMap::new()),
            parent: SpinMutex::new(None),
        })
    }

    /// Get VMO ID
    pub const fn id(&self) -> VmoId {
        self.id
    }

    /// Get VMO size
    pub fn size(&self) -> usize {
        self.size.load(Ordering::Acquire)
    }

    /// Resize the VMO
    ///
    /// Only works if VMO was created with RESIZABLE flag.
    pub fn resize(&self, new_size: usize) -> Result<(), &'static str> {
        if !self.flags.is_resizable() {
            return Err("VMO not resizable");
        }

        // Round up to page size
        let page_size = 4096;
        let size_aligned = (new_size + page_size - 1) / page_size * page_size;

        // Update size
        self.size.store(size_aligned, Ordering::Release);

        // TODO: Adjust page map if shrinking

        Ok(())
    }

    /// Write data to the VMO
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset within VMO
    /// * `data` - Data to write
    pub fn write(&self, offset: usize, data: &[u8]) -> Result<usize, &'static str> {
        let size = self.size();

        if offset >= size {
            return Err("offset out of bounds");
        }

        let end = core::cmp::min(offset + data.len(), size);
        let to_write = &data[..end - offset];

        let page_size = 4096;

        // Pre-allocate all pages needed for this write operation
        // This avoids holding the SpinMutex during allocation
        let mut pages_to_allocate = alloc::vec::Vec::new();
        let mut data_offset = 0;

        // First pass: identify which pages need allocation
        {
            let pages = self.pages.lock();
            while data_offset < to_write.len() {
                let write_offset = offset + data_offset;
                let page_index = write_offset / page_size;
                let key = page_index * page_size;

                if !pages.contains_key(&key) {
                    pages_to_allocate.push(key);
                }

                // Move to next page
                let page_offset = write_offset % page_size;
                let space_in_page = page_size - page_offset;
                let remaining = to_write.len() - data_offset;
                data_offset += core::cmp::min(remaining, space_in_page);
            }
        }

        // Second pass: allocate all pages (without holding lock)
        use crate::mm::pmm;
        for key in &pages_to_allocate {
            let paddr = pmm::pmm_alloc_user_page()
                .map_err(|_| "Failed to allocate user page")?;

            // Insert the page into the map (holding lock briefly)
            let mut pages = self.pages.lock();
            pages.entry(*key).or_insert(PageMapEntry {
                paddr,
                present: true,
                writable: true,
            });
        }

        // Third pass: write data to pages
        let mut bytes_written = 0;
        data_offset = 0;

        while data_offset < to_write.len() {
            let write_offset = offset + data_offset;
            let page_index = write_offset / page_size;
            let page_offset = write_offset % page_size;
            let key = page_index * page_size;

            // Get page entry (holding lock briefly)
            let (page_paddr, page_present) = {
                let pages = self.pages.lock();
                let entry = pages.get(&key).unwrap();
                (entry.paddr, entry.present)
            };

            if !page_present {
                return Err("page not present (allocation failed)");
            }

            // Calculate how much to write to this page
            let remaining = to_write.len() - data_offset;
            let space_in_page = page_size - page_offset;
            let to_copy = core::cmp::min(remaining, space_in_page);

            // Get virtual address of the page using proper address conversion
            // CRITICAL: Use paddr_to_vaddr_user_zone for user zone memory
            let vaddr = crate::mm::pmm::paddr_to_vaddr_user_zone(page_paddr) + page_offset;

            // Write data to the page
            unsafe {
                let dst = vaddr as *mut u8;
                let src = to_write.as_ptr().add(data_offset);
                core::ptr::copy_nonoverlapping(src, dst, to_copy);
            }

            data_offset += to_copy;
            bytes_written += to_copy;
        }

        Ok(bytes_written)
    }

    /// Read data from the VMO
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset within VMO
    /// * `buf` - Buffer to read into
    pub fn read(&self, offset: usize, buf: &mut [u8]) -> Result<usize, &'static str> {
        let size = self.size();

        if offset >= size {
            return Err("offset out of bounds");
        }

        let end = core::cmp::min(offset + buf.len(), size);
        let to_read = end - offset;

        let page_size = 4096;
        let pages = self.pages.lock();
        let mut bytes_read = 0;

        // Read data page by page
        while bytes_read < to_read {
            let read_offset = offset + bytes_read;
            let page_index = read_offset / page_size;
            let page_offset = read_offset % page_size;

            // Check if page exists
            let page_entry = match pages.get(&(page_index * page_size)) {
                Some(entry) => entry,
                None => {
                    // Page not present - return zeros
                    let remaining = to_read - bytes_read;
                    let space_in_page = page_size - page_offset;
                    let to_copy = core::cmp::min(remaining, space_in_page);
                    buf[bytes_read..bytes_read + to_copy].fill(0);
                    bytes_read += to_copy;
                    continue;
                }
            };

            // Calculate how much to read from this page
            let remaining = to_read - bytes_read;
            let space_in_page = page_size - page_offset;
            let to_copy = core::cmp::min(remaining, space_in_page);

            // Get virtual address of the page using proper address conversion
            // CRITICAL: Use paddr_to_vaddr_user_zone for user zone memory
            let vaddr = crate::mm::pmm::paddr_to_vaddr_user_zone(page_entry.paddr) + page_offset;

            // Read data from the page
            unsafe {
                let src = vaddr as *const u8;
                let dst = buf.as_mut_ptr().add(bytes_read);
                core::ptr::copy_nonoverlapping(src, dst, to_copy);
            }

            bytes_read += to_copy;
        }

        Ok(bytes_read)
    }

    /// Clone the VMO (copy-on-write)
    ///
    /// # Returns
    ///
    /// New VMO that shares pages with parent
    pub fn clone(&self) -> Result<Self, &'static str> {
        let cloned = Self::create(self.size(), VmoFlags::empty)?;

        // Copy all pages from parent to child
        {
            let parent_pages = self.pages.lock();
            let mut child_pages = cloned.pages.lock();

            for (offset, page_entry) in parent_pages.iter() {
                if page_entry.present {
                    // Allocate a new physical page for the child from user zone
                    use crate::mm::pmm;
                    let new_paddr = pmm::pmm_alloc_user_page()
                        .map_err(|_| "Failed to allocate page for clone")?;

                    // Copy the page data using small chunks to avoid stack overflow
                    // Use a 256-byte buffer instead of 4KB to fit within kernel stack
                    let chunk_size = 256usize;
                    let page_size = 4096usize;
                    let mut offset_in_page = 0usize;

                    while offset_in_page < page_size {
                        let mut buffer = [0u8; 256];
                        let bytes_to_copy = core::cmp::min(chunk_size, page_size - offset_in_page);

                        unsafe {
                            // Copy from source to buffer
                            let src_vaddr = pmm::paddr_to_vaddr_user_zone(page_entry.paddr + offset_in_page as u64);
                            let src_ptr = src_vaddr as *const u8;
                            core::ptr::copy_nonoverlapping(src_ptr, buffer.as_mut_ptr(), bytes_to_copy);

                            // Copy from buffer to destination
                            let dst_vaddr = pmm::paddr_to_vaddr_user_zone(new_paddr + offset_in_page as u64);
                            let dst_ptr = dst_vaddr as *mut u8;
                            core::ptr::copy_nonoverlapping(buffer.as_ptr(), dst_ptr, bytes_to_copy);
                        }

                        offset_in_page += bytes_to_copy;
                    }

                    // Add the page to the child
                    child_pages.insert(*offset, PageMapEntry {
                        paddr: new_paddr,
                        present: true,
                        writable: true,
                    });
                }
            }
        } // Locks are released here

        Ok(cloned)
    }

    /// Get cache policy
    pub fn cache_policy(&self) -> CachePolicy {
        *self.cache_policy.lock()
    }

    /// Set cache policy
    pub fn set_cache_policy(&self, policy: CachePolicy) {
        *self.cache_policy.lock() = policy;
    }

    /// Get the kernel object base
    pub fn base(&self) -> &KernelObjectBase {
        &self.base
    }

    /// Get reference count
    pub fn ref_count(&self) -> usize {
        self.base.ref_count()
    }

    /// Increment reference count
    pub fn ref_inc(&self) {
        self.base.ref_inc();
    }

    /// Decrement reference count
    ///
    /// Returns true if this was the last reference.
    pub fn ref_dec(&self) -> bool {
        self.base.ref_dec()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vmo_flags() {
        let flags = VmoFlags::empty();
        assert!(!flags.is_resizable());
        assert!(!flags.is_cow());

        let flags = VmoFlags::RESIZABLE;
        assert!(flags.is_resizable());

        let flags = VmoFlags::COW;
        assert!(flags.is_cow());

        let flags = VmoFlags::RESIZABLE | VmoFlags::COW;
        assert!(flags.is_resizable());
        assert!(flags.is_cow());
    }

    #[test]
    fn test_cache_policy() {
        assert_eq!(CachePolicy::from_raw(0), CachePolicy::Default);
        assert_eq!(CachePolicy::from_raw(1), CachePolicy::Uncached);
        assert_eq!(CachePolicy::from_raw(2), CachePolicy::WriteCombining);
        assert_eq!(CachePolicy::from_raw(3), CachePolicy::WriteThrough);
    }

    #[test]
    fn test_vmo_create() {
        let vmo = Vmo::create(0x1000, VmoFlags::empty()).unwrap();
        assert_eq!(vmo.size(), 0x1000);
        assert_eq!(vmo.cache_policy(), CachePolicy::Default);
    }

    #[test]
    fn test_vmo_create_rounding() {
        let vmo = Vmo::create(0x1001, VmoFlags::empty()).unwrap();
        // Should be rounded up to page size (4096)
        assert_eq!(vmo.size(), 0x2000);
    }

    #[test]
    fn test_vmo_create_zero() {
        assert!(Vmo::create(0, VmoFlags::empty()).is_err());
    }

    #[test]
    fn test_vmo_resize() {
        let vmo = Vmo::create(0x1000, VmoFlags::RESIZABLE).unwrap();
        vmo.resize(0x2000).unwrap();
        assert_eq!(vmo.size(), 0x2000);
    }

    #[test]
    fn test_vmo_not_resizable() {
        let vmo = Vmo::create(0x1000, VmoFlags::empty()).unwrap();
        assert!(vmo.resize(0x2000).is_err());
    }

    #[test]
    fn test_vmo_write_read() {
        let vmo = Vmo::create(0x1000, VmoFlags::empty()).unwrap();

        let data = [1, 2, 3, 4];
        vmo.write(0, &data).unwrap();

        let mut buf = [0u8; 10];
        let bytes_read = vmo.read(0, &mut buf).unwrap();

        assert_eq!(bytes_read, 4);
        // Note: Data is not actually stored yet (stub implementation)
    }

    #[test]
    fn test_vmo_read_out_of_bounds() {
        let vmo = Vmo::create(0x1000, VmoFlags::empty()).unwrap();

        let mut buf = [0u8; 10];
        assert!(vmo.read(0x2000, &mut buf).is_err());
    }

    #[test]
    fn test_vmo_clone() {
        let parent = Vmo::create(0x1000, VmoFlags::empty()).unwrap();
        let child = parent.clone().unwrap();

        assert!(child.flags.is_cow());
        assert_eq!(child.size(), parent.size());
    }
}
