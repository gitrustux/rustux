// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! USB device handles (step 13): a USB mass-storage interface that the
//! kernel's xHCI driver set up, for the userspace usb-storage driver.
//! The handle names the device by an id that is never reused; once the
//! device is unplugged every call on it fails with PEER_CLOSED.

use super::SignalState;

pub struct UsbDevice {
    pub signals: SignalState,
    pub id: u32,
}

impl UsbDevice {
    pub fn new(id: u32) -> alloc::sync::Arc<Self> {
        alloc::sync::Arc::new(Self { signals: SignalState::new(0), id })
    }
}
