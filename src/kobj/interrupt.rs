// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Resources and interrupt objects: what a userspace driver needs.
//!
//! - The **root resource** is the capability to touch hardware. The kernel
//!   gives it to `init`, which hands it to the drivers it starts.
//! - An **interrupt object** binds a legacy ISA IRQ to a handle. When the
//!   IRQ fires, the kernel's handler only records it and wakes the thread
//!   in `interrupt_wait`; the driver talks to the device itself. Closing the
//!   last handle unbinds the IRQ.
//!
//! The interrupt handler touches only atomics: interrupts arrive while a
//! thread runs in user mode or the CPU idles, never inside kernel code that
//! holds locks.

use super::SignalState;
use crate::arch::amd64::mm::RxStatus;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};

/// The capability to use hardware (I/O ports, interrupts)
pub struct Resource {
    pub signals: SignalState,
}

static ROOT_RESOURCE: crate::sync::SpinMutex<Option<Arc<Resource>>> = crate::sync::SpinMutex::new(None);

impl Resource {
    pub fn root() -> Arc<Resource> {
        ROOT_RESOURCE.lock().get_or_insert_with(|| Arc::new(Resource { signals: SignalState::new(0) })).clone()
    }
}

/// ISA IRQs that can be bound. Only IRQ 1 (PS/2 keyboard) is wired to the
/// interrupt handlers so far.
pub const BINDABLE_IRQS: &[u32] = &[1];

const ISA_IRQS: usize = 16;

#[allow(clippy::declare_interior_mutable_const)]
const NO_BINDING: AtomicPtr<Interrupt> = AtomicPtr::new(core::ptr::null_mut());
/// Interrupt object bound to each ISA IRQ (not owning: cleared by Drop)
static BOUND: [AtomicPtr<Interrupt>; ISA_IRQS] = [NO_BINDING; ISA_IRQS];

pub struct Interrupt {
    pub signals: SignalState,
    pub irq: u32,
    /// Fired since the last successful wait
    triggered: AtomicBool,
    /// Monotonic ns of the last firing
    timestamp: AtomicU64,
    /// Thread blocked in interrupt_wait (0 = none)
    waiter: AtomicU32,
    destroyed: AtomicBool,
}

impl Interrupt {
    /// Bind a new interrupt object to ISA `irq`
    pub fn bind(irq: u32) -> Result<Arc<Self>, RxStatus> {
        if !BINDABLE_IRQS.contains(&irq) {
            return Err(RxStatus::ERR_NOT_SUPPORTED);
        }
        let i = Arc::new(Self {
            signals: SignalState::new(0),
            irq,
            triggered: AtomicBool::new(false),
            timestamp: AtomicU64::new(0),
            waiter: AtomicU32::new(0),
            destroyed: AtomicBool::new(false),
        });
        let ptr = Arc::as_ptr(&i) as *mut Interrupt;
        BOUND[irq as usize]
            .compare_exchange(core::ptr::null_mut(), ptr, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| RxStatus::ERR_ALREADY_EXISTS)?;
        Ok(i)
    }

    /// Called from the IRQ handler. Returns false if nothing is bound
    /// (the kernel then leaves the device alone).
    pub fn dispatch(irq: u32) -> bool {
        let p = match BOUND.get(irq as usize) {
            Some(slot) => slot.load(Ordering::Acquire),
            None => return false,
        };
        if p.is_null() {
            return false;
        }
        // Safe: Drop clears the slot before the object goes away, and Drop
        // runs in kernel code with interrupts disabled
        let i = unsafe { &*p };
        i.timestamp.store(super::now_ns(), Ordering::Release);
        i.triggered.store(true, Ordering::Release);
        let w = i.waiter.load(Ordering::Acquire);
        if w != 0 {
            crate::process::run::wake(w);
        }
        true
    }

    /// Block until the IRQ fires (or the object is destroyed / the thread
    /// killed). Returns the firing's timestamp.
    pub fn wait(&self) -> Result<u64, RxStatus> {
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
            // Only one thread may wait (as in Zircon)
            if self.waiter.compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire).map_or_else(|w| w != tid, |_| false) {
                return Err(RxStatus::ERR_BAD_STATE);
            }
            if !self.triggered.load(Ordering::Acquire) && !self.destroyed.load(Ordering::Acquire) {
                crate::process::run::block_current(super::INFINITE);
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

    /// interrupt_ack: the line is never masked for edge-triggered ISA IRQs,
    /// so there is nothing to re-enable yet
    pub fn ack(&self) -> Result<(), RxStatus> {
        if self.destroyed.load(Ordering::Acquire) {
            return Err(RxStatus::ERR_CANCELED);
        }
        Ok(())
    }
}

impl Drop for Interrupt {
    fn drop(&mut self) {
        let me = self as *mut Interrupt;
        let _ = BOUND[self.irq as usize].compare_exchange(me, core::ptr::null_mut(), Ordering::AcqRel, Ordering::Acquire);
    }
}
