// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Resources and interrupt objects: what a userspace driver needs.
//!
//! - The **root resource** is the capability to touch hardware. The kernel
//!   gives it to `init`, which hands it to the drivers it starts.
//! - An **interrupt object** binds an interrupt vector to a handle: a
//!   legacy ISA IRQ (vector 32 + IRQ) or an MSI vector from a pool
//!   (0x40-0x7F) that a PCI device is programmed to send. When it fires,
//!   the kernel's handler only records it and wakes the thread in
//!   `interrupt_wait`; the driver talks to the device itself. Closing the
//!   last handle frees the vector.
//!
//! The interrupt handler touches only atomics: interrupts arrive while a
//! thread runs in user mode or the CPU idles, never inside kernel code that
//! holds locks.

use super::SignalState;
use crate::arch::amd64::mm::RxStatus;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// A capability over the system. The root resource allows everything
/// (hardware access, debug log, clock, power); `resource_create` makes
/// narrower ones from it, such as the power resource the shell gets.
pub struct Resource {
    pub signals: SignalState,
    pub kind: u32,
    /// For IOPORT: the ports [base, base + size) it allows
    pub base: u64,
    pub size: u64,
}

/// Resource kinds
pub mod kind {
    pub const ROOT: u32 = 0;
    /// Restart and power off (`system_powerctl`)
    pub const POWER: u32 = 1;
    /// A range of I/O ports (`ioports_request`), for a driver whose device
    /// has an I/O BAR
    pub const IOPORT: u32 = 2;
}

static ROOT_RESOURCE: crate::sync::SpinMutex<Option<Arc<Resource>>> = crate::sync::SpinMutex::new(None);

impl Resource {
    pub fn root() -> Arc<Resource> {
        ROOT_RESOURCE.lock().get_or_insert_with(|| Arc::new(Resource { signals: SignalState::new(0), kind: kind::ROOT, base: 0, size: 0 })).clone()
    }

    pub fn new(kind: u32) -> Arc<Resource> {
        Arc::new(Resource { signals: SignalState::new(0), kind, base: 0, size: 0 })
    }

    pub fn new_range(kind: u32, base: u64, size: u64) -> Arc<Resource> {
        Arc::new(Resource { signals: SignalState::new(0), kind, base, size })
    }
}

/// ISA IRQs that can be bound. Only IRQ 1 (PS/2 keyboard) is wired to the
/// interrupt handlers so far.
pub const BINDABLE_IRQS: &[u32] = &[1];

/// Vector of ISA IRQ n
pub const ISA_VECTOR_BASE: u32 = 32;
/// MSI vectors handed out by `bind_msi` (each has a handler, arch/amd64/msi.rs)
pub const MSI_FIRST: u32 = 0x40;
pub const MSI_COUNT: u32 = 64;

/// Interrupt object bound to each vector, as an address (not owning:
/// cleared by Drop). Locked so that the handler on one CPU and Drop on
/// another cannot overlap.
static BOUND: crate::sync::IrqMutex<[usize; 256]> = crate::sync::IrqMutex::new([0; 256]);

/// What an MSI or MSI-X capability must be programmed with
#[derive(Clone, Copy)]
pub struct MsiInfo {
    pub address: u64,
    pub data: u32,
    pub vector: u32,
}

pub struct Interrupt {
    pub signals: SignalState,
    /// ISA IRQ, or 0 for MSI
    pub irq: u32,
    pub vector: u32,
    pub msi: bool,
    /// Fired since the last successful wait
    triggered: AtomicBool,
    /// Monotonic ns of the last firing
    timestamp: AtomicU64,
    /// Thread blocked in interrupt_wait (0 = none)
    waiter: AtomicU32,
    destroyed: AtomicBool,
}

impl Interrupt {
    fn make(irq: u32, vector: u32, msi: bool) -> Arc<Self> {
        Arc::new(Self {
            signals: SignalState::new(0),
            irq,
            vector,
            msi,
            triggered: AtomicBool::new(false),
            timestamp: AtomicU64::new(0),
            waiter: AtomicU32::new(0),
            destroyed: AtomicBool::new(false),
        })
    }

    /// Bind a new interrupt object to ISA `irq`
    pub fn bind(irq: u32) -> Result<Arc<Self>, RxStatus> {
        if !BINDABLE_IRQS.contains(&irq) {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        let vector = ISA_VECTOR_BASE + irq;
        let i = Self::make(irq, vector, false);
        let mut bound = BOUND.lock();
        if bound[vector as usize] != 0 {
            return Err(RxStatus::ERR_ALREADY_EXISTS);
        }
        bound[vector as usize] = Arc::as_ptr(&i) as usize;
        drop(bound);
        Ok(i)
    }

    /// A new interrupt object on a free MSI vector (NO_RESOURCES when all
    /// 64 are taken)
    pub fn bind_msi() -> Result<Arc<Self>, RxStatus> {
        let mut bound = BOUND.lock();
        let v = (MSI_FIRST..MSI_FIRST + MSI_COUNT).find(|&v| bound[v as usize] == 0).ok_or(RxStatus::ERR_NO_RESOURCES)?;
        let i = Self::make(0, v, true);
        bound[v as usize] = Arc::as_ptr(&i) as usize;
        drop(bound);
        Ok(i)
    }

    /// Address and data for the device: a fixed-delivery, edge-triggered
    /// message to the boot CPU's local APIC
    pub fn msi_info(&self) -> Option<MsiInfo> {
        if !self.msi {
            return None;
        }
        let apic = crate::arch::amd64::percpu::get(0).map(|c| c.apic_id()).unwrap_or(0);
        Some(MsiInfo { address: 0xFEE0_0000 | ((apic as u64 & 0xFF) << 12), data: self.vector, vector: self.vector })
    }

    /// Called from the ISA IRQ handlers
    pub fn dispatch(irq: u32) -> bool {
        Self::dispatch_vector(ISA_VECTOR_BASE + irq)
    }

    /// Called from the interrupt handlers. Returns false if nothing is
    /// bound (the kernel then leaves the device alone).
    pub fn dispatch_vector(vector: u32) -> bool {
        let bound = BOUND.lock();
        let p = match bound.get(vector as usize) {
            Some(&p) if p != 0 => p as *const Interrupt,
            _ => return false,
        };
        // Safe: Drop clears the slot (under this lock) before the object
        // goes away
        let i = unsafe { &*p };
        i.timestamp.store(super::now_ns(), Ordering::Release);
        i.triggered.store(true, Ordering::Release);
        let w = i.waiter.load(Ordering::Acquire);
        drop(bound);
        if w != 0 {
            crate::process::run::wake(w);
        }
        true
    }

    /// Block until the IRQ fires (or the object is destroyed / the thread
    /// killed, or monotonic time reaches `deadline`: TIMED_OUT). Returns
    /// the firing's timestamp.
    pub fn wait(&self, deadline: u64) -> Result<u64, RxStatus> {
        let (tid, _) = crate::process::run::current_thread().ok_or(RxStatus::ERR_BAD_STATE)?;
        loop {
            if self.destroyed.load(Ordering::Acquire) {
                return Err(RxStatus::ERR_CANCELED);
            }
            if self.triggered.swap(false, Ordering::AcqRel) {
                return Ok(self.timestamp.load(Ordering::Acquire));
            }
            if crate::process::run::is_killed() {
                return Err(RxStatus::ERR_CANCELED);
            }
            if deadline != super::INFINITE && super::now_ns() >= deadline {
                return Err(RxStatus::ERR_TIMED_OUT);
            }
            // Only one thread may wait (as in Zircon)
            if self.waiter.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire).map_or_else(|w| w != tid, |_| false) {
                return Err(RxStatus::ERR_BAD_STATE);
            }
            if !self.triggered.load(Ordering::Acquire) && !self.destroyed.load(Ordering::Acquire) {
                crate::process::run::block_current(deadline);
            }
            self.waiter.store(0, Ordering::Release);
        }
    }

    /// Wake a waiter with ERR_CANCELED; later waits fail too
    pub fn destroy(&self) {
        self.destroyed.store(true, Ordering::Release);
        let w = self.waiter.load(Ordering::Acquire);
        if w != 0 {
            crate::process::run::wake(w);
        }
    }

    /// interrupt_ack: ISA IRQs and MSIs are edge-triggered and never
    /// masked, so there is nothing to re-enable
    pub fn ack(&self) -> Result<(), RxStatus> {
        if self.destroyed.load(Ordering::Acquire) {
            return Err(RxStatus::ERR_CANCELED);
        }
        Ok(())
    }
}

impl Drop for Interrupt {
    fn drop(&mut self) {
        let me = self as *mut Interrupt as usize;
        let mut bound = BOUND.lock();
        if bound[self.vector as usize] == me {
            bound[self.vector as usize] = 0;
        }
    }
}
