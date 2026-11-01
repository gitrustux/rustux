// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! xHCI rings in DMA memory: producer rings (command and transfer rings,
//! with a Link TRB in the last slot) and the event ring (consumer).

use crate::mm::dma::{dma_alloc, DmaBuffer, DmaZone};
use super::{Result, UsbError};
use core::sync::atomic::{fence, Ordering};

/// A Transfer Request Block
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Trb {
    pub param: u64,
    pub status: u32,
    pub control: u32,
}

pub const TRB_CYCLE: u32 = 1 << 0;
pub const TRB_TOGGLE_CYCLE: u32 = 1 << 1;
pub const TRB_TYPE_LINK: u32 = 6;

/// TRBs per one-page ring
pub const RING_TRBS: usize = 4096 / 16;

/// Allocate one zeroed page for the controller. Below 4 GiB, so it also
/// works with controllers that cannot do 64-bit DMA.
pub fn dma_page() -> Result<DmaBuffer> {
    dma_alloc(4096, DmaZone::Below4G).map_err(|_| UsbError::NoMemory)
}

/// Producer ring (command ring or a transfer ring), one page
pub struct Ring {
    buf: DmaBuffer,
    enqueue: usize,
    cycle: u32,
}

impl Ring {
    pub fn new() -> Result<Self> {
        Ok(Self { buf: dma_page()?, enqueue: 0, cycle: 1 })
    }

    /// Physical address of the first TRB
    pub fn phys(&self) -> u64 {
        self.buf.phys
    }

    /// Empty the ring for reuse
    pub fn reset(&mut self) {
        unsafe { core::ptr::write_bytes(self.buf.virt, 0, 4096) };
        self.enqueue = 0;
        self.cycle = 1;
    }

    fn slot(&self, i: usize) -> *mut Trb {
        unsafe { (self.buf.virt as *mut Trb).add(i) }
    }

    /// Write a TRB so the controller sees it only once complete: first with
    /// the cycle bit it does not own, then flip the control dword.
    unsafe fn write(&self, i: usize, param: u64, status: u32, control: u32) {
        let control = control & !TRB_CYCLE;
        core::ptr::write_volatile(self.slot(i), Trb { param, status, control: control | (self.cycle ^ 1) });
        fence(Ordering::SeqCst);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*self.slot(i)).control), control | self.cycle);
        fence(Ordering::SeqCst);
    }

    /// Enqueue a TRB; returns its physical address. The Link TRB back to
    /// the start is written when the ring wraps.
    pub fn push(&mut self, param: u64, status: u32, control: u32) -> u64 {
        unsafe {
            if self.enqueue >= RING_TRBS - 1 {
                self.write(RING_TRBS - 1, self.buf.phys, 0, (TRB_TYPE_LINK << 10) | TRB_TOGGLE_CYCLE);
                self.enqueue = 0;
                self.cycle ^= 1;
            }
            let i = self.enqueue;
            self.write(i, param, status, control);
            self.enqueue += 1;
            self.buf.phys + (i * 16) as u64
        }
    }

    /// Where the controller resumes after the last TRB pushed, and the
    /// cycle state there (for a command ring abort)
    pub fn next(&self) -> (u64, u32) {
        (self.buf.phys + (self.enqueue * 16) as u64, self.cycle)
    }
}

/// Event ring segment, one page, with its one-entry segment table
pub struct EventRing {
    buf: DmaBuffer,
    pub erst: DmaBuffer,
    dequeue: usize,
    cycle: u32,
}

impl EventRing {
    pub fn new() -> Result<Self> {
        let buf = dma_page()?;
        let erst = dma_page()?;
        unsafe {
            // ERST entry: segment base, size (in TRBs)
            core::ptr::write_volatile(erst.virt as *mut u64, buf.phys);
            core::ptr::write_volatile((erst.virt as *mut u32).add(2), RING_TRBS as u32);
        }
        Ok(Self { buf, erst, dequeue: 0, cycle: 1 })
    }

    pub fn phys(&self) -> u64 {
        self.buf.phys
    }

    /// Physical address of the next TRB to read (for ERDP)
    pub fn dequeue_phys(&self) -> u64 {
        self.buf.phys + (self.dequeue * 16) as u64
    }

    /// Take the next event, if the controller has written one
    pub fn pop(&mut self) -> Option<Trb> {
        let trb = unsafe { core::ptr::read_volatile((self.buf.virt as *const Trb).add(self.dequeue)) };
        if trb.control & TRB_CYCLE != self.cycle {
            return None;
        }
        fence(Ordering::Acquire);
        self.dequeue += 1;
        if self.dequeue == RING_TRBS {
            self.dequeue = 0;
            self.cycle ^= 1;
        }
        Some(trb)
    }
}
