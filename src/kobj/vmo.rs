// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Virtual memory objects: a resizable-later, page-granular piece of memory
//! that can be read, written and mapped into address spaces.
//!
//! Pages are committed (allocated and zeroed) on first write: through
//! `vmo_write`, or a mapping's first touch (the page fault handler,
//! `Aspace::fault_in`: demand paging, DESKTOP.md step F1); reading an
//! uncommitted page gives zeros. Pages are freed when the last handle and
//! the last mapping are gone.
//!
//! Two more kinds serve drivers (step 12):
//! - **physical**: a fixed range of device memory (registers, a frame
//!   buffer), made from the root resource; never RAM, never freed, and not
//!   readable through `vmo_read`/`vmo_write` (map it instead);
//! - **contiguous**: RAM allocated in one physically contiguous run, for
//!   DMA buffers a device walks without scatter lists.
//!
//! And one for the kernel itself: **wired**, RAM that already holds data
//! (the boot image inside the kernel) handed out without copying; its
//! pages are never freed.
//!
//! Each VMO has a cache policy used when it is mapped (uncached for
//! device registers).

use super::SignalState;
use crate::arch::amd64::mm::{kernel_map, RxStatus};
use crate::mm::pmm;
use crate::sync::SpinMutex;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

pub const PAGE: u64 = 4096;
/// Largest VMO (keeps the page list small; raise when VMOs get page trees)
pub const MAX_SIZE: u64 = 1 << 30;

/// Cache policies (Zircon's ZX_CACHE_POLICY_* values)
pub mod cache {
    pub const CACHED: u32 = 0;
    pub const UNCACHED: u32 = 1;
    pub const UNCACHED_DEVICE: u32 = 2;
    pub const WRITE_COMBINING: u32 = 3;
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VmoKind {
    Paged,
    Physical,
    Contiguous,
    Wired,
}

/// Pages per chunk of a [`PageList`] (2 MiB of memory per 4 KiB chunk)
const CHUNK: usize = 512;

/// The physical address of each page of a VMO (0: not committed), in
/// chunks allocated when one of their pages is first committed: a 1 GiB
/// VMO costs 4 KiB of list until it is used (demand paging, step F1)
pub struct PageList {
    chunks: Vec<Option<alloc::boxed::Box<[u64; CHUNK]>>>,
    len: usize,
}

impl PageList {
    fn new(len: usize) -> Result<Self, RxStatus> {
        let mut chunks = Vec::new();
        let n = len.div_ceil(CHUNK);
        chunks.try_reserve_exact(n).map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        chunks.resize_with(n, || None);
        Ok(Self { chunks, len })
    }

    /// All `len` pages at once (physical, wired, contiguous VMOs)
    fn filled(len: usize, page: impl Fn(usize) -> u64) -> Result<Self, RxStatus> {
        let mut l = Self::new(len)?;
        for i in 0..len {
            l.set(i, page(i));
        }
        Ok(l)
    }

    fn len(&self) -> usize {
        self.len
    }

    fn get(&self, i: usize) -> u64 {
        match &self.chunks[i / CHUNK] {
            Some(c) => c[i % CHUNK],
            None => 0,
        }
    }

    fn set(&mut self, i: usize, p: u64) {
        let c = self.chunks[i / CHUNK].get_or_insert_with(|| alloc::boxed::Box::new([0u64; CHUNK]));
        c[i % CHUNK] = p;
    }

    /// Committed pages among [first, last)
    fn count(&self, first: usize, last: usize) -> usize {
        (first..last.min(self.len)).filter(|&i| self.chunks[i / CHUNK].is_some() && self.get(i) != 0).count()
    }

    fn for_each_committed(&self, mut f: impl FnMut(u64)) {
        for c in self.chunks.iter().flatten() {
            for &p in c.iter() {
                if p != 0 {
                    f(p);
                }
            }
        }
    }
}

pub struct Vmo {
    pub signals: SignalState,
    size: u64,
    /// Physical address of each page, 0 = not committed
    pages: SpinMutex<PageList>,
    pub kind: VmoKind,
    cache: AtomicU32,
}

fn round_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE - 1).map(|x| x & !(PAGE - 1))
}

impl Vmo {
    pub fn new(size: u64) -> Result<Arc<Self>, RxStatus> {
        let size = round_up(size).ok_or(RxStatus::ERR_OUT_OF_RANGE)?;
        if size > MAX_SIZE {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        let pages = PageList::new((size / PAGE) as usize)?;
        Ok(Arc::new(Self::with(size, pages, VmoKind::Paged)))
    }

    fn with(size: u64, pages: PageList, kind: VmoKind) -> Self {
        Self { signals: SignalState::new(0), size, pages: SpinMutex::new(pages), kind, cache: AtomicU32::new(cache::CACHED) }
    }

    /// A VMO over device memory [base, base + size); the caller has checked
    /// that the range is not RAM. Uncached until set otherwise.
    pub fn new_physical(base: u64, size: u64) -> Result<Arc<Self>, RxStatus> {
        if base % PAGE != 0 || size == 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let size = round_up(size).ok_or(RxStatus::ERR_OUT_OF_RANGE)?;
        if size > MAX_SIZE || base.checked_add(size).is_none() {
            return Err(RxStatus::ERR_OUT_OF_RANGE);
        }
        let pages = PageList::filled((size / PAGE) as usize, |i| base + i as u64 * PAGE)?;
        let v = Self::with(size, pages, VmoKind::Physical);
        v.cache.store(cache::UNCACHED_DEVICE, Ordering::Relaxed);
        Ok(Arc::new(v))
    }

    /// RAM at [base, base + size) that already holds data and stays
    /// allocated for good (part of the kernel image), as a VMO without a
    /// copy. Safety: the range is page aligned, RAM, and never freed or
    /// reused; whoever gets the VMO can read all of it.
    pub unsafe fn new_wired(base: u64, size: u64) -> Result<Arc<Self>, RxStatus> {
        if base % PAGE != 0 || size % PAGE != 0 || size == 0 || size > MAX_SIZE {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let pages = PageList::filled((size / PAGE) as usize, |i| base + i as u64 * PAGE)?;
        Ok(Arc::new(Self::with(size, pages, VmoKind::Wired)))
    }

    /// Zeroed RAM in one contiguous run, aligned to 2^`align_log2` bytes
    /// (at least a page); `low` keeps it below 4 GiB for 32-bit DMA
    pub fn new_contiguous(size: u64, align_log2: u8, low: bool) -> Result<Arc<Self>, RxStatus> {
        let size = round_up(size).ok_or(RxStatus::ERR_OUT_OF_RANGE)?;
        if size == 0 || size > MAX_SIZE || align_log2 > 30 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let count = (size / PAGE) as usize;
        let mut pages = PageList::new(count)?;
        let mut flags = pmm::PMM_ALLOC_FLAG_USER;
        if low {
            flags |= pmm::PMM_ALLOC_FLAG_LOW_MEM;
        }
        let base = pmm::pmm_alloc_contiguous(count, flags, align_log2).map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        if low && base + size > 1 << 32 {
            pmm::pmm_free_contiguous(base, count);
            return Err(RxStatus::ERR_NO_MEMORY);
        }
        for i in 0..count as u64 {
            let p = base + i * PAGE;
            unsafe { core::ptr::write_bytes(kernel_map::phys_to_virt(p) as *mut u8, 0, PAGE as usize) };
            pages.set(i as usize, p);
        }
        Ok(Arc::new(Self::with(size, pages, VmoKind::Contiguous)))
    }

    pub fn cache_policy(&self) -> u32 {
        self.cache.load(Ordering::Acquire)
    }

    /// Set the cache policy for later mappings (write-combining needs the
    /// PAT, which is not set up: NOT_SUPPORTED)
    pub fn set_cache_policy(&self, policy: u32) -> Result<(), RxStatus> {
        match policy {
            cache::CACHED | cache::UNCACHED | cache::UNCACHED_DEVICE => {
                self.cache.store(policy, Ordering::Release);
                Ok(())
            }
            cache::WRITE_COMBINING => Err(RxStatus::ERR_NOT_SUPPORTED),
            _ => Err(RxStatus::ERR_INVALID_ARGS),
        }
    }

    /// Physical addresses of the pages in [offset, offset+len) (page
    /// aligned), committing them; for pinning
    pub fn phys_pages(&self, offset: u64, len: u64) -> Result<Vec<u64>, RxStatus> {
        if offset % PAGE != 0 || len % PAGE != 0 || len == 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        self.commit_range(offset, len)
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    fn check_range(&self, offset: u64, len: u64) -> Result<(), RxStatus> {
        match offset.checked_add(len) {
            Some(end) if end <= self.size => Ok(()),
            _ => Err(RxStatus::ERR_OUT_OF_RANGE),
        }
    }

    fn commit(pages: &mut PageList, index: usize) -> Result<u64, RxStatus> {
        if pages.get(index) == 0 {
            let p = pmm::pmm_alloc_user_page().map_err(|_| RxStatus::ERR_NO_MEMORY)?;
            unsafe { core::ptr::write_bytes(kernel_map::phys_to_virt(p) as *mut u8, 0, PAGE as usize) };
            pages.set(index, p);
        }
        Ok(pages.get(index))
    }

    /// Copy `len` bytes at `offset` to `dst`.
    ///
    /// # Safety
    /// `dst` must be valid for `len` bytes of writes.
    pub unsafe fn read_to(&self, offset: u64, dst: *mut u8, len: u64) -> Result<(), RxStatus> {
        if self.kind == VmoKind::Physical {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        self.check_range(offset, len)?;
        let pages = self.pages.lock();
        let mut done = 0u64;
        while done < len {
            let pos = offset + done;
            let in_page = pos % PAGE;
            let n = (PAGE - in_page).min(len - done);
            let p = pages.get((pos / PAGE) as usize);
            if p == 0 {
                core::ptr::write_bytes(dst.add(done as usize), 0, n as usize);
            } else {
                let src = (kernel_map::phys_to_virt(p) + in_page) as *const u8;
                core::ptr::copy(src, dst.add(done as usize), n as usize);
            }
            done += n;
        }
        Ok(())
    }

    /// Copy `len` bytes from `src` to `offset`, committing pages as needed.
    ///
    /// # Safety
    /// `src` must be valid for `len` bytes of reads.
    pub unsafe fn write_from(&self, offset: u64, src: *const u8, len: u64) -> Result<(), RxStatus> {
        if self.kind == VmoKind::Physical {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        self.check_range(offset, len)?;
        let mut pages = self.pages.lock();
        let mut done = 0u64;
        while done < len {
            let pos = offset + done;
            let in_page = pos % PAGE;
            let n = (PAGE - in_page).min(len - done);
            let p = Self::commit(&mut pages, (pos / PAGE) as usize)?;
            let dst = (kernel_map::phys_to_virt(p) + in_page) as *mut u8;
            core::ptr::copy(src.add(done as usize), dst, n as usize);
            done += n;
        }
        Ok(())
    }

    /// Commit the pages of a page-aligned range and return their physical
    /// addresses (for mapping)
    pub fn commit_range(&self, offset: u64, len: u64) -> Result<Vec<u64>, RxStatus> {
        self.check_range(offset, len)?;
        let mut pages = self.pages.lock();
        let first = (offset / PAGE) as usize;
        let count = (len / PAGE) as usize;
        let mut out = Vec::new();
        out.try_reserve_exact(count).map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        for i in first..first + count {
            out.push(Self::commit(&mut pages, i)?);
        }
        Ok(out)
    }

    /// The pages of a page-aligned range as they are: physical addresses,
    /// 0 for pages not committed yet (nothing is committed)
    pub fn present_pages(&self, offset: u64, len: u64) -> Result<Vec<u64>, RxStatus> {
        self.check_range(offset, len)?;
        let pages = self.pages.lock();
        let first = (offset / PAGE) as usize;
        let count = (len / PAGE) as usize;
        let mut out = Vec::new();
        out.try_reserve_exact(count).map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        out.extend((first..first + count).map(|i| pages.get(i)));
        Ok(out)
    }

    /// Commit one page (page-aligned `offset`) and return it
    pub fn commit_page(&self, offset: u64) -> Result<u64, RxStatus> {
        self.check_range(offset, PAGE)?;
        let mut pages = self.pages.lock();
        Self::commit(&mut pages, (offset / PAGE) as usize)
    }

    /// A new VMO holding a copy of `size` bytes from page-aligned `offset`
    /// (bytes past the end of this VMO read as zero). Later writes to
    /// either VMO are not seen by the other.
    pub fn snapshot(&self, offset: u64, size: u64) -> Result<Arc<Vmo>, RxStatus> {
        if offset % PAGE != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        if self.kind == VmoKind::Physical {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        let child = Vmo::new(size)?;
        let src = self.pages.lock();
        let mut dst = child.pages.lock();
        for i in 0..dst.len() {
            let j = (offset / PAGE) as usize + i;
            if j >= src.len() {
                break;
            }
            let p = src.get(j);
            if p != 0 {
                let q = Self::commit(&mut dst, i)?;
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        kernel_map::phys_to_virt(p) as *const u8,
                        kernel_map::phys_to_virt(q) as *mut u8,
                        PAGE as usize,
                    )
                };
            }
        }
        drop(dst);
        Ok(child)
    }

    /// Number of committed pages (for tests and debugging)
    pub fn committed(&self) -> usize {
        if self.kind == VmoKind::Physical {
            return 0;
        }
        let pages = self.pages.lock();
        pages.count(0, pages.len())
    }

    /// Committed pages in [offset, offset + len)
    pub fn committed_in(&self, offset: u64, len: u64) -> usize {
        if self.kind == VmoKind::Physical {
            return 0;
        }
        let first = (offset / PAGE) as usize;
        let last = offset.saturating_add(len).div_ceil(PAGE) as usize;
        self.pages.lock().count(first, last)
    }
}

impl Drop for Vmo {
    fn drop(&mut self) {
        if self.kind == VmoKind::Physical || self.kind == VmoKind::Wired {
            return;
        }
        self.pages.lock().for_each_committed(|p| {
            pmm::pmm_free_page(p);
        });
    }
}
