// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Virtual memory objects: a resizable-later, page-granular piece of memory
//! that can be read, written and mapped into address spaces.
//!
//! Pages are committed (allocated and zeroed) on first write or when
//! mapped; reading an uncommitted page gives zeros. Pages are freed when
//! the last handle and the last mapping are gone.

use super::SignalState;
use crate::arch::amd64::mm::{kernel_map, RxStatus};
use crate::mm::pmm;
use crate::sync::SpinMutex;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const PAGE: u64 = 4096;
/// Largest VMO (keeps the page list small; raise when VMOs get page trees)
pub const MAX_SIZE: u64 = 1 << 30;

pub struct Vmo {
    pub signals: SignalState,
    size: u64,
    /// Physical address of each page, 0 = not committed
    pages: SpinMutex<Vec<u64>>,
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
        let mut pages = Vec::new();
        pages.try_reserve_exact((size / PAGE) as usize).map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        pages.resize((size / PAGE) as usize, 0);
        Ok(Arc::new(Self { signals: SignalState::new(0), size, pages: SpinMutex::new(pages) }))
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

    fn commit(pages: &mut [u64], index: usize) -> Result<u64, RxStatus> {
        if pages[index] == 0 {
            let p = pmm::pmm_alloc_user_page().map_err(|_| RxStatus::ERR_NO_MEMORY)?;
            unsafe { core::ptr::write_bytes(kernel_map::phys_to_virt(p) as *mut u8, 0, PAGE as usize) };
            pages[index] = p;
        }
        Ok(pages[index])
    }

    /// Copy `len` bytes at `offset` to `dst`.
    ///
    /// # Safety
    /// `dst` must be valid for `len` bytes of writes.
    pub unsafe fn read_to(&self, offset: u64, dst: *mut u8, len: u64) -> Result<(), RxStatus> {
        self.check_range(offset, len)?;
        let pages = self.pages.lock();
        let mut done = 0u64;
        while done < len {
            let pos = offset + done;
            let in_page = pos % PAGE;
            let n = (PAGE - in_page).min(len - done);
            let p = pages[(pos / PAGE) as usize];
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

    /// A new VMO holding a copy of `size` bytes from page-aligned `offset`
    /// (bytes past the end of this VMO read as zero). Later writes to
    /// either VMO are not seen by the other.
    pub fn snapshot(&self, offset: u64, size: u64) -> Result<Arc<Vmo>, RxStatus> {
        if offset % PAGE != 0 {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let child = Vmo::new(size)?;
        let src = self.pages.lock();
        let mut dst = child.pages.lock();
        for i in 0..dst.len() {
            let Some(&p) = src.get((offset / PAGE) as usize + i) else { break };
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
        self.pages.lock().iter().filter(|&&p| p != 0).count()
    }
}

impl Drop for Vmo {
    fn drop(&mut self) {
        for &p in self.pages.lock().iter() {
            if p != 0 {
                pmm::pmm_free_page(p);
            }
        }
    }
}
