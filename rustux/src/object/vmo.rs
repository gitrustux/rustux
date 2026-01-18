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
struct PageMapEntry {
    /// Physical page address
    paddr: PAddr,

    /// Whether page is present (not committed if COW)
    present: bool,

    /// Whether page is writable
    writable: bool,
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

        // TODO: Implement actual memory writing
        // For now, this is a stub

        Ok(to_write.len())
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

        // TODO: Implement actual memory reading
        // For now, this is a stub
        for i in 0..to_read {
            buf[i] = 0;
        }

        Ok(to_read)
    }

    /// Clone the VMO (copy-on-write)
    ///
    /// # Returns
    ///
    /// New VMO that shares pages with parent
    pub fn clone(&self) -> Result<Self, &'static str> {
        let flags = VmoFlags::COW;
        let cloned = Self::create(self.size(), flags)?;

        // Set parent (interior mutability through Mutex)
        *cloned.parent.lock() = Some(self as *const _);

        // Increment parent ref count
        self.base.ref_inc();

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
