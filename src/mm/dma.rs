// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! DMA buffer allocation
//!
//! Devices that read or write memory themselves (xHCI rings and contexts,
//! disk and network descriptors) need buffers that are:
//!
//! - physically contiguous: the device sees physical addresses only;
//! - aligned and not crossing certain boundaries (xHCI: 64-byte aligned
//!   rings, structures that must not cross a 64 KiB boundary);
//! - below 4 GiB for devices that only drive 32-bit addresses;
//! - zeroed, so stale data is never handed to a device.
//!
//! A [`DmaBuffer`] carries both addresses: `phys` to program into the
//! device, `virt` (in the kernel's direct map, valid in every address
//! space) for the CPU. On x86 DMA is cache-coherent, so the normal
//! write-back mapping is fine.

use crate::arch::amd64::mm::{PAddr, RxResult, RxStatus};
use crate::mm::pmm;

const PAGE_SIZE: usize = 4096;

/// Where a DMA buffer may live
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaZone {
    /// Anywhere the device can reach with 64-bit addresses
    Any,
    /// Below 4 GiB, for devices limited to 32-bit addresses
    Below4G,
}

/// A physically contiguous, zeroed buffer for device DMA
#[derive(Debug)]
pub struct DmaBuffer {
    /// Physical address (for the device)
    pub phys: PAddr,
    /// Kernel virtual address (for the CPU)
    pub virt: *mut u8,
    /// Size in pages
    pub pages: usize,
}

impl DmaBuffer {
    /// Size in bytes
    pub fn len(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    /// The buffer as a byte slice
    ///
    /// # Safety
    /// The caller must make sure the device is not writing it concurrently.
    pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        core::slice::from_raw_parts_mut(self.virt, self.len())
    }
}

/// Allocate a zeroed DMA buffer of at least `bytes` bytes, page aligned.
pub fn dma_alloc(bytes: usize, zone: DmaZone) -> RxResult<DmaBuffer> {
    dma_alloc_aligned(bytes, PAGE_SIZE, zone)
}

/// Allocate a zeroed DMA buffer of at least `bytes` bytes whose physical
/// address is a multiple of `align` (a power of two; at least a page).
///
/// To keep a structure from crossing an N-byte boundary, allocate it with
/// `align = N` when it is at most N bytes long.
pub fn dma_alloc_aligned(bytes: usize, align: usize, zone: DmaZone) -> RxResult<DmaBuffer> {
    if bytes == 0 || !align.is_power_of_two() {
        return Err(RxStatus::ERR_INVALID_ARGS);
    }
    let pages = (bytes + PAGE_SIZE - 1) / PAGE_SIZE;
    let align_log2 = core::cmp::max(align, PAGE_SIZE).trailing_zeros() as u8;
    let flags = match zone {
        DmaZone::Any => pmm::PMM_ALLOC_FLAG_KERNEL,
        DmaZone::Below4G => pmm::PMM_ALLOC_FLAG_KERNEL | pmm::PMM_ALLOC_FLAG_LOW_MEM,
    };
    let phys = pmm::pmm_alloc_contiguous(pages, flags, align_log2)?;
    if zone == DmaZone::Below4G && phys + (pages * PAGE_SIZE) as PAddr > 0x1_0000_0000 {
        // Only possible with the fallback zones, which have no 4 GiB split
        let _ = pmm::pmm_free_contiguous(phys, pages);
        return Err(RxStatus::ERR_NO_MEMORY);
    }
    let virt = pmm::paddr_to_vaddr(phys) as *mut u8;
    unsafe { core::ptr::write_bytes(virt, 0, pages * PAGE_SIZE) };
    Ok(DmaBuffer { phys, virt, pages })
}

/// Return a DMA buffer to the PMM. The device must no longer use it.
pub fn dma_free(buf: DmaBuffer) {
    let _ = pmm::pmm_free_contiguous(buf.phys, buf.pages);
}

/// Boot-time check of the allocator; prints one `[DMA]` line on the debug
/// port and returns whether every check passed.
pub fn dma_self_test() -> bool {
    fn out(s: &str) {
        for &b in s.as_bytes() {
            unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack)) };
        }
    }
    let free_before = pmm::pmm_count_free_pages();
    let mut ok = true;

    // 3 pages below 4 GiB, 64 KiB aligned: zeroed, reachable through virt,
    // and the same memory through the direct map of phys
    match dma_alloc_aligned(3 * PAGE_SIZE, 64 * 1024, DmaZone::Below4G) {
        Ok(mut b) => {
            let (phys, len) = (b.phys, b.len());
            ok &= phys % (64 * 1024) == 0;
            ok &= phys + len as PAddr <= 0x1_0000_0000;
            let s = unsafe { b.as_mut_slice() };
            ok &= s.iter().all(|&x| x == 0);
            s[0] = 0xA5;
            s[len - 1] = 0x5A;
            let alias = pmm::paddr_to_vaddr(phys) as *const u8;
            ok &= unsafe { *alias == 0xA5 && *alias.add(len - 1) == 0x5A };
            dma_free(b);
        }
        Err(_) => ok = false,
    }
    // A plain single-page buffer
    match dma_alloc(100, DmaZone::Any) {
        Ok(b) => {
            ok &= b.pages == 1 && b.phys % PAGE_SIZE as PAddr == 0;
            dma_free(b);
        }
        Err(_) => ok = false,
    }
    ok &= pmm::pmm_count_free_pages() == free_before;

    out(if ok { "[DMA] Self-test passed\n" } else { "[DMA] SELF-TEST FAILED\n" });
    ok
}
