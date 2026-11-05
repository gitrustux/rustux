// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Address spaces and virtual memory address regions (VMARs).
//!
//! Each process has an [`Aspace`]: its page tables plus the list of VMO
//! mappings made through VMAR handles. A mapping holds a reference to its
//! VMO, so the memory stays alive while it is mapped even if every VMO
//! handle is closed.
//!
//! A process's root VMAR covers [`ROOT_BASE`, `ROOT_END`): all of user space
//! except the first 64 KiB, so null-pointer accesses fault. The program
//! image and stack set up by the kernel's loader are ordinary mappings in
//! it. Mappings without a fixed address go at or above [`ALLOC_HINT`] when
//! there is room, away from where programs are linked.

use super::{vmo::PAGE, SignalState, Vmo};
use crate::arch::amd64::mm::{kernel_map, RxStatus};
use crate::sync::SpinMutex;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub const ROOT_BASE: u64 = 0x0000_0000_0001_0000;
pub const ROOT_END: u64 = 0x0000_7fff_ffff_f000;
/// Where mappings without a fixed address start (4 GiB)
pub const ALLOC_HINT: u64 = 0x0000_0001_0000_0000;

/// vmar_map / vmar_protect options (Zircon values)
pub mod opts {
    pub const PERM_READ: u32 = 1 << 0;
    pub const PERM_WRITE: u32 = 1 << 1;
    pub const PERM_EXECUTE: u32 = 1 << 2;
    pub const SPECIFIC: u32 = 1 << 4;
    pub const PERMS: u32 = PERM_READ | PERM_WRITE | PERM_EXECUTE;
}

pub struct Mapping {
    pub size: u64,
    pub vmo: Arc<Vmo>,
    pub vmo_offset: u64,
    pub perms: u32,
    /// Highest permissions allowed (from the handles used to map it)
    pub max_perms: u32,
}

struct Inner {
    /// Set when the process exits; no more mappings can be made
    dead: bool,
    /// Mappings by start address
    maps: BTreeMap<u64, Mapping>,
}

pub struct Aspace {
    pml4: u64,
    inner: SpinMutex<Inner>,
}

fn pte_flags(perms: u32) -> u64 {
    let mut f = kernel_map::PTE_USER;
    if perms & opts::PERM_WRITE != 0 {
        f |= kernel_map::PTE_WRITABLE;
    }
    if perms & opts::PERM_EXECUTE == 0 {
        f |= kernel_map::nx_flag();
    }
    f
}

/// x86 cannot map a page that is writable or executable but not readable
pub fn perms_valid(perms: u32) -> bool {
    perms & !opts::PERMS == 0 && (perms == 0 || perms & opts::PERM_READ != 0)
}

fn alloc_table_page() -> Option<u64> {
    crate::mm::pmm::pmm_alloc_kernel_page().ok()
}

impl Aspace {
    pub fn new(pml4: u64) -> Arc<Self> {
        Arc::new(Self { pml4, inner: SpinMutex::new(Inner { dead: false, maps: BTreeMap::new() }) })
    }

    /// Map `len` bytes of `vmo` from `vmo_offset` inside [lo, hi), at
    /// `addr` if given, else at the lowest free address. Returns the address.
    pub fn map(&self, lo: u64, hi: u64, addr: Option<u64>, vmo: Arc<Vmo>, vmo_offset: u64, len: u64,
               perms: u32, max_perms: u32) -> Result<u64, RxStatus> {
        if len == 0 || vmo_offset % PAGE != 0 || !perms_valid(perms) {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let len = len.checked_add(PAGE - 1).ok_or(RxStatus::ERR_OUT_OF_RANGE)? & !(PAGE - 1);
        match vmo_offset.checked_add(len) {
            Some(end) if end <= vmo.size() => {}
            _ => return Err(RxStatus::ERR_OUT_OF_RANGE),
        }
        let mut inner = self.inner.lock();
        if inner.dead {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        let base = match addr {
            Some(a) => {
                if a % PAGE != 0 || a < lo || a.checked_add(len).map_or(true, |e| e > hi) {
                    return Err(RxStatus::ERR_INVALID_ARGS);
                }
                if overlaps(&inner.maps, a, a + len) {
                    return Err(RxStatus::ERR_ALREADY_EXISTS);
                }
                a
            }
            None => {
                let from_hint = if lo < ALLOC_HINT && ALLOC_HINT < hi { find_gap(&inner.maps, ALLOC_HINT, hi, len) } else { None };
                from_hint.or_else(|| find_gap(&inner.maps, lo, hi, len)).ok_or(RxStatus::ERR_NO_MEMORY)?
            }
        };
        let pages = vmo.commit_range(vmo_offset, len)?;
        if perms != 0 {
            let flags = pte_flags(perms);
            for (i, &p) in pages.iter().enumerate() {
                let va = base + i as u64 * PAGE;
                if unsafe { kernel_map::map_4k(self.pml4, va, p, flags, alloc_table_page) }.is_err() {
                    for j in 0..i {
                        unsafe { kernel_map::unmap_4k(self.pml4, base + j as u64 * PAGE) };
                    }
                    return Err(RxStatus::ERR_NO_MEMORY);
                }
            }
        }
        inner.maps.insert(base, Mapping { size: len, vmo, vmo_offset, perms, max_perms });
        Ok(base)
    }

    /// Remove mappings in [addr, addr+len); mappings that cross the edges
    /// are split. Returns the removed mappings so the caller drops them
    /// (and maybe their VMOs) outside the lock.
    pub fn unmap(&self, addr: u64, len: u64) -> Result<Vec<Mapping>, RxStatus> {
        let end = range_end(addr, len)?;
        let mut inner = self.inner.lock();
        if inner.dead {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        split_at(&mut inner.maps, addr);
        split_at(&mut inner.maps, end);
        let keys: Vec<u64> = inner.maps.range(addr..end).map(|(&k, _)| k).collect();
        if keys.is_empty() {
            return Err(RxStatus::ERR_NOT_FOUND);
        }
        let mut removed = Vec::new();
        for k in keys {
            let m = inner.maps.remove(&k).unwrap();
            for i in 0..m.size / PAGE {
                unsafe { kernel_map::unmap_4k(self.pml4, k + i * PAGE) };
            }
            removed.push(m);
        }
        drop(inner);
        // Other CPUs running this address space must stop using the old
        // translations before the caller frees the pages
        crate::arch::amd64::smp::tlb_shootdown(self.pml4);
        Ok(removed)
    }

    /// Change the permissions of [addr, addr+len), which must be fully
    /// covered by mappings
    pub fn protect(&self, addr: u64, len: u64, perms: u32) -> Result<(), RxStatus> {
        if !perms_valid(perms) {
            return Err(RxStatus::ERR_INVALID_ARGS);
        }
        let end = range_end(addr, len)?;
        let mut inner = self.inner.lock();
        if inner.dead {
            return Err(RxStatus::ERR_BAD_STATE);
        }
        // Check coverage and permissions before changing anything
        let mut covered_to = addr;
        for (&k, m) in inner.maps.range(..end) {
            if k + m.size <= addr {
                continue;
            }
            if k > covered_to {
                break;
            }
            if perms & !m.max_perms != 0 {
                return Err(RxStatus::ERR_ACCESS_DENIED);
            }
            covered_to = k + m.size;
        }
        if covered_to < end {
            return Err(RxStatus::ERR_NOT_FOUND);
        }
        split_at(&mut inner.maps, addr);
        split_at(&mut inner.maps, end);
        let pml4 = self.pml4;
        let mut remap = Vec::new();
        for (&k, m) in inner.maps.range_mut(addr..end) {
            let was = m.perms;
            m.perms = perms;
            for i in 0..m.size / PAGE {
                let va = k + i * PAGE;
                if perms == 0 {
                    unsafe { kernel_map::unmap_4k(pml4, va) };
                } else if was == 0 {
                    remap.push((va, m.vmo.clone(), m.vmo_offset + i * PAGE));
                } else {
                    unsafe { kernel_map::protect_4k(pml4, va, pte_flags(perms)) };
                }
            }
        }
        drop(inner);
        crate::arch::amd64::smp::tlb_shootdown(pml4);
        // Pages of a mapping that had no access were never put in the page
        // tables; they were committed when it was mapped
        for (va, vmo, off) in remap {
            let p = vmo.commit_range(off, PAGE)?[0];
            unsafe { kernel_map::map_4k(pml4, va, p, pte_flags(perms), alloc_table_page) }
                .map_err(|_| RxStatus::ERR_NO_MEMORY)?;
        }
        Ok(())
    }

    /// The process is gone: drop every mapping (returned so the caller drops
    /// them outside locks) and refuse new ones. The page tables are not
    /// touched; they are never used again.
    pub fn destroy(&self) -> BTreeMap<u64, Mapping> {
        let mut inner = self.inner.lock();
        inner.dead = true;
        core::mem::take(&mut inner.maps)
    }

    /// (bytes mapped, bytes of memory committed in the VMOs mapped, each
    /// VMO counted once)
    pub fn stats(&self) -> (u64, u64) {
        let inner = self.inner.lock();
        let mapped = inner.maps.values().map(|m| m.size).sum();
        let mut seen: Vec<*const Vmo> = Vec::new();
        let mut committed = 0u64;
        for m in inner.maps.values() {
            let p = Arc::as_ptr(&m.vmo);
            if !seen.contains(&p) {
                seen.push(p);
                committed += m.vmo.committed() as u64 * PAGE;
            }
        }
        (mapped, committed)
    }

    pub fn mapping_count(&self) -> usize {
        self.inner.lock().maps.len()
    }
}

fn range_end(addr: u64, len: u64) -> Result<u64, RxStatus> {
    if addr % PAGE != 0 || len == 0 {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    let len = len.checked_add(PAGE - 1).ok_or(RxStatus::ERR_INVALID_ARGS)? & !(PAGE - 1);
    addr.checked_add(len).ok_or(RxStatus::ERR_INVALID_ARGS)
}

fn overlaps(maps: &BTreeMap<u64, Mapping>, start: u64, end: u64) -> bool {
    if let Some((&k, m)) = maps.range(..end).next_back() {
        return k + m.size > start;
    }
    false
}

fn find_gap(maps: &BTreeMap<u64, Mapping>, lo: u64, hi: u64, len: u64) -> Option<u64> {
    let mut cand = lo;
    for (&k, m) in maps.range(..hi) {
        if k + m.size <= cand {
            continue;
        }
        if k >= cand && k - cand >= len {
            break;
        }
        cand = k + m.size;
    }
    match cand.checked_add(len) {
        Some(e) if e <= hi => Some(cand),
        _ => None,
    }
}

/// Make sure no mapping crosses `addr` (split the one that does)
fn split_at(maps: &mut BTreeMap<u64, Mapping>, addr: u64) {
    let Some((&k, m)) = maps.range_mut(..addr).next_back() else { return };
    if k + m.size <= addr {
        return;
    }
    let front = addr - k;
    let tail = Mapping {
        size: m.size - front,
        vmo: m.vmo.clone(),
        vmo_offset: m.vmo_offset + front,
        perms: m.perms,
        max_perms: m.max_perms,
    };
    m.size = front;
    maps.insert(addr, tail);
}

/// A region of an address space that mappings can be made in
pub struct Vmar {
    pub signals: SignalState,
    pub aspace: Arc<Aspace>,
    pub base: u64,
    pub size: u64,
}

impl Vmar {
    pub fn root(aspace: Arc<Aspace>) -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0), aspace, base: ROOT_BASE, size: ROOT_END - ROOT_BASE })
    }

    pub fn end(&self) -> u64 {
        self.base + self.size
    }

    /// Whether [addr, addr+len) lies inside this region
    pub fn contains(&self, addr: u64, len: u64) -> bool {
        addr >= self.base && addr.checked_add(len).map_or(false, |e| e <= self.end())
    }
}
