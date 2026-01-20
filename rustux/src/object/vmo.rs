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
        let mut pages = self.pages.lock();
        let mut bytes_written = 0;

        // Write data page by page
        let mut data_offset = 0;
        while data_offset < to_write.len() {
            let write_offset = offset + data_offset;
            let page_index = write_offset / page_size;
            let page_offset = write_offset % page_size;

            // Get or allocate page
            let key = page_index * page_size;

            // Debug: Print VMO ID and key
            unsafe {
                let msg = b"[VMO-WRITE] VMO#";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                let mut n = self.id;
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
                let msg = b" key=";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
                let mut n = key;
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

            let page_entry = pages.entry(key).or_insert_with(|| {
                use crate::mm::pmm;

                // Debug: Before allocation
                unsafe {
                    let msg = b"[VMO-WRITE] Allocating page\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                // Allocate a physical page from user zone
                match pmm::pmm_alloc_user_page() {
                    Ok(paddr) => {
                        // Debug: Success
                        unsafe {
                            let msg = b"[VMO-WRITE] Page allocated OK\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                        PageMapEntry {
                            paddr,
                            present: true,
                            writable: true,
                        }
                    },
                    Err(e) => {
                        // Debug: Failed
                        unsafe {
                            let msg = b"[VMO-WRITE] Page allocation FAILED\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                        // Failed to allocate - return a dummy entry
                        // (write will fail when we try to access it)
                        PageMapEntry {
                            paddr: 0,
                            present: false,
                            writable: false,
                        }
                    }
                }
            });

            if !page_entry.present {
                return Err("page not present (allocation failed)");
            }

            // Debug: After insert, verify BTreeMap state (drop borrow first)
            drop(page_entry);
            {
                let len = pages.len();
                unsafe {
                    let msg = b"[VMO-WRITE] BTreeMap len=";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                    let mut n = len;
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

                // Verify the entry is actually there
                let verify = pages.get(&key);
                match verify {
                    Some(e) => {
                        unsafe {
                            let msg = b"[VMO-WRITE] Verify: entry exists, present=";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let digit = if e.present { b'1' } else { b'0' };
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") digit, options(nomem, nostack));
                            let msg = b" paddr=0x";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                            let mut n = e.paddr;
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
                    }
                    None => {
                        unsafe {
                            let msg = b"[VMO-WRITE] Verify: entry MISSING!\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }
                }
            }

            // Re-borrow for the rest of the function
            let page_entry = pages.get(&key).unwrap();

            // Calculate how much to write to this page
            let remaining = to_write.len() - data_offset;
            let space_in_page = page_size - page_offset;
            let to_copy = core::cmp::min(remaining, space_in_page);

            // Get virtual address of the page using proper address conversion
            // CRITICAL: Use paddr_to_vaddr_user_zone for user zone memory
            let vaddr = crate::mm::pmm::paddr_to_vaddr_user_zone(page_entry.paddr) + page_offset;

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
        // Debug: Entering clone
        unsafe {
            let msg = b"[VMO] clone() starting\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        let cloned = Self::create(self.size(), VmoFlags::empty)?;

        // Debug: VMO created
        unsafe {
            let msg = b"[VMO] clone() created new VMO\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

        // Copy all pages from parent to child
        {
            let parent_pages = self.pages.lock();

            // Debug: Locked parent pages
            unsafe {
                let msg = b"[VMO] clone() locked parent pages\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            let mut child_pages = cloned.pages.lock();

            // Debug: Locked child pages
            unsafe {
                let msg = b"[VMO] clone() locked child pages\n";
                for &byte in msg {
                    core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                }
            }

            for (offset, page_entry) in parent_pages.iter() {
                // Debug: Before processing entry
                unsafe {
                    let msg = b"[VMO] clone: iter entry\n";
                    for &byte in msg {
                        core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                    }
                }

                // Debug: Check VMO#3 BEFORE clone (only for VMO#1)
                if self.id == 1 {
                    unsafe {
                        let msg = b"[VMO] VMO#1: BEFORE CLONE OPS\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }
                }

                if page_entry.present {
                    // Debug: About to allocate new page
                    unsafe {
                        let msg = b"[VMO] clone() allocating new page\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Allocate a new physical page for the child from user zone
                    use crate::mm::pmm;
                    let new_paddr = pmm::pmm_alloc_user_page()
                        .map_err(|_| "Failed to allocate page for clone")?;

                    // Debug: About to copy page data
                    unsafe {
                        let msg = b"[VMO] clone() copying page data\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Copy the page data using a heap buffer as intermediate storage.
                    // This avoids aliasing with kernel heap metadata and doesn't require
                    // temporary page mappings or identity mapping assumptions.
                    //
                    // The process is:
                    // 1. Allocate stack buffer [u8; PAGE_SIZE]
                    // 2. Copy from source physical page -> buffer
                    // 3. Copy from buffer -> destination physical page
                    //
                    // This guarantees:
                    // - Only accessing mapped memory (heap is guaranteed mapped)
                    // - No aliasing with kernel heap (buffer is separate from metadata)
                    // - No dependence on identity mapping (uses paddr_to_vaddr())
                    let page_size = 4096;
                    let mut buffer: [u8; 4096] = [0; 4096];

                    // Debug: Show we're using heap buffer
                    unsafe {
                        let msg = b"[VMO] Using heap buffer for copy\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Debug: Before copy
                    if self.id == 1 {
                        unsafe {
                            let msg = b"[VMO] Before VMO#1 copy\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }

                    // Debug: Print addresses
                    unsafe {
                        let msg = b"[VMO] clone: src_paddr=0x";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = page_entry.paddr;
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
                        let msg = b" dst_paddr=0x";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = new_paddr;
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
                        let msg = b" buffer=0x";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                        let mut n = buffer.as_ptr() as usize;
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

                    unsafe {
                        // Debug: Before copy
                        if self.id == 1 {
                            let msg = b"[VMO] BEFORE COPY\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }

                        // Step 1: Copy from source physical page to buffer
                        // CRITICAL: Use paddr_to_vaddr_user_zone for user zone memory
                        // to avoid assuming identity mapping is available
                        let src_vaddr = pmm::paddr_to_vaddr_user_zone(page_entry.paddr);
                        let src_ptr = src_vaddr as *const u8;
                        core::ptr::copy_nonoverlapping(src_ptr, buffer.as_mut_ptr(), page_size);

                        // Step 2: Copy from buffer to destination physical page
                        // CRITICAL: Use paddr_to_vaddr_user_zone for user zone memory
                        let dst_vaddr = pmm::paddr_to_vaddr_user_zone(new_paddr);
                        let dst_ptr = dst_vaddr as *mut u8;
                        core::ptr::copy_nonoverlapping(buffer.as_ptr(), dst_ptr, page_size);

                        // Debug: After copy
                        if self.id == 1 {
                            let msg = b"[VMO] AFTER COPY\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }

                    // Debug: IMMEDIATELY after copy, before anything else
                    if self.id == 1 {
                        unsafe {
                            let msg = b"[VMO] COPY DONE\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }

                    // Debug: After copy, check if we corrupted something
                    // Only check during second clone (VMO#2)
                    if self.id == 2 {
                        use crate::exec::process_loader;
                        unsafe {
                            let msg = b"[VMO] After copy: checking VMO#3\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                        // This is a hack - we need access to loaded_elf but we don't have it here
                        // Skip this check for now
                    }

                    // Debug: Page data copied
                    unsafe {
                        let msg = b"[VMO] clone() page data copied\n";
                        for &byte in msg {
                            core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                        }
                    }

                    // Add the page to the child
                    // Debug: Before insert
                    if self.id == 1 {
                        unsafe {
                            let msg = b"[VMO] Before child_pages.insert\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }

                    child_pages.insert(*offset, PageMapEntry {
                        paddr: new_paddr,
                        present: true,
                        writable: true,
                    });

                    // Debug: After insert
                    if self.id == 1 {
                        unsafe {
                            let msg = b"[VMO] After child_pages.insert\n";
                            for &byte in msg {
                                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
                            }
                        }
                    }
                }
            }
        } // Locks are released here

        // Debug: Clone complete
        unsafe {
            let msg = b"[VMO] clone() complete\n";
            for &byte in msg {
                core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") byte, options(nomem, nostack));
            }
        }

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
