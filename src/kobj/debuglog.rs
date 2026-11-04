// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Debug log objects (Zircon's `zx_debuglog`). Writing to one prints on the
//! kernel console (screen) and the debug port. Creating one needs the root
//! resource, so only `init` (the console service) and processes it trusts
//! can print directly; every other program prints through its stdout
//! handle. Reading the log back (for `dmesg`) comes in step 9.

use super::SignalState;
use alloc::sync::Arc;

/// Largest write accepted at once
pub const MAX_WRITE: usize = 4096;

pub struct DebugLog {
    pub signals: SignalState,
}

impl DebugLog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { signals: SignalState::new(0) })
    }

    pub fn write(&self, bytes: &[u8]) {
        crate::drivers::display::write_bytes(bytes);
        for &b in bytes {
            unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack, preserves_flags)) };
        }
    }
}
