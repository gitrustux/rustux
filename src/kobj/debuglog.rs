// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Debug log objects (Zircon's `zx_debuglog`). Writing to one prints on the
//! kernel console (screen) and the debug port. Creating one needs the root
//! resource, so only `init` (the console service) and processes it trusts
//! can print directly; every other program prints through its stdout
//! handle. A read-only debug log object reads the kernel log (`dmesg`).

use super::SignalState;
use alloc::sync::Arc;

/// Largest write accepted at once
pub const MAX_WRITE: usize = 4096;

pub struct DebugLog {
    pub signals: SignalState,
    /// Position in the kernel log of the next read
    pos: core::sync::atomic::AtomicU64,
}

impl DebugLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0), pos: core::sync::atomic::AtomicU64::new(0) })
    }

    /// Read kernel log bytes not read yet through this object
    pub fn read(&self, out: &mut [u8]) -> usize {
        use core::sync::atomic::Ordering;
        let mut pos = self.pos.load(Ordering::Acquire);
        let n = crate::klog::read(&mut pos, out);
        self.pos.store(pos, Ordering::Release);
        n
    }

    pub fn write(&self, bytes: &[u8]) {
        crate::drivers::display::write_bytes(bytes);
        for &b in bytes {
            unsafe { crate::klog::port_put(b) };
        }
    }
}
