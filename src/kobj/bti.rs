// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Bus transaction initiators and pinned memory (Zircon's BTI and PMT).
//!
//! A **BTI** stands for one device's view of memory when it does DMA. A
//! driver gets one from the device manager and uses it to:
//! - allocate contiguous buffers (`vmo_create_contiguous`), and
//! - **pin** pages of a VMO (`bti_pin`), which returns the addresses the
//!   device must use and keeps the pages in place until `pmt_unpin`.
//!
//! Without an IOMMU the device addresses are the physical addresses. With
//! one (later), the BTI would select the device's translation tables and
//! pinning would map the pages there, so drivers already do the right
//! thing.

use super::{SignalState, Vmo};
use crate::sync::SpinMutex;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct Bti {
    pub signals: SignalState,
    /// Which device (the device manager uses bus:device.function)
    pub bti_id: u64,
    /// PMTs not unpinned yet
    pub pinned: AtomicUsize,
}

impl Bti {
    pub fn new(bti_id: u64) -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0), bti_id, pinned: AtomicUsize::new(0) })
    }
}

/// Pinned pages: holding the VMO keeps its pages (VMO pages never move or
/// get decommitted while it lives)
pub struct Pmt {
    pub signals: SignalState,
    bti: Arc<Bti>,
    vmo: SpinMutex<Option<Arc<Vmo>>>,
    pub offset: u64,
    pub size: u64,
}

impl Pmt {
    pub fn new(bti: Arc<Bti>, vmo: Arc<Vmo>, offset: u64, size: u64) -> Arc<Self> {
        bti.pinned.fetch_add(1, Ordering::AcqRel);
        Arc::new(Self { signals: SignalState::new(0), bti, vmo: SpinMutex::new(Some(vmo)), offset, size })
    }

    /// Release the pages (also done when the last handle goes)
    pub fn unpin(&self) {
        if self.vmo.lock().take().is_some() {
            self.bti.pinned.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for Pmt {
    fn drop(&mut self) {
        self.unpin();
    }
}
