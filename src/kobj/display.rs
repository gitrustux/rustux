// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! The boot framebuffer for the display service (DESKTOP.md step F2).
//!
//! The kernel found the framebuffer through UEFI GOP at boot and draws its
//! text console there. `display_claim` (root resource) hands it to one
//! process at a time: a physical VMO over it, mapped write-combining, its
//! geometry, and a claim handle. Through the claim the holder switches the
//! screen between its graphics and the text console (`display_console`);
//! when the claim's last handle closes (the display service ended, or was
//! killed) the console is shown again and the framebuffer can be claimed
//! anew. A kernel panic shows the console whatever the claim says.

use super::SignalState;
use crate::sync::BootCell;
use core::sync::atomic::{AtomicBool, Ordering};

/// The framebuffer as GOP described it (set once at boot)
#[derive(Clone, Copy)]
pub struct BootFb {
    pub addr: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Pixels per scan line (>= width)
    pub stride: u32,
    /// 32-bit pixel masks (GOP PixelBlueGreenRedReserved is red 0xFF0000,
    /// green 0xFF00, blue 0xFF)
    pub red: u32,
    pub green: u32,
    pub blue: u32,
}

static BOOT_FB: BootCell<BootFb> =
    BootCell::new(BootFb { addr: 0, size: 0, width: 0, height: 0, stride: 0, red: 0, green: 0, blue: 0 });

/// Record the GOP framebuffer (boot, one CPU)
pub fn set_boot_framebuffer(fb: BootFb) {
    unsafe { *BOOT_FB.boot_mut() = fb };
}

pub fn boot_framebuffer() -> Option<BootFb> {
    let fb = unsafe { *BOOT_FB.get() };
    if fb.addr == 0 { None } else { Some(fb) }
}

/// Set while a claim exists
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// The right to the screen; see the module comment
pub struct DisplayClaim {
    pub signals: SignalState,
}

impl DisplayClaim {
    /// The one claim, unless someone holds it
    pub fn take() -> Option<alloc::sync::Arc<Self>> {
        if CLAIMED.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(alloc::sync::Arc::new(Self { signals: SignalState::new(0) }))
    }
}

impl Drop for DisplayClaim {
    fn drop(&mut self) {
        // The display service is gone: the text console has the screen
        crate::drivers::display::show();
        CLAIMED.store(false, Ordering::Release);
    }
}
