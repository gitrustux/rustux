// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! USB host support: xHCI controller, USB 2.0 hubs, HID boot keyboards.
//!
//! Ported from the `kernel-efi` driver that was brought up on the test
//! laptop (MSI GS75, Intel `8086:06ED`, keyboards behind a KVM hub), with
//! the kernel's own memory model: device structures come from the DMA
//! allocator (physical address for the controller, direct-map address for
//! the CPU) and the controller's registers are mapped uncached through
//! `kernel_map::map_mmio`.
//!
//! - [`init`] finds the chipset xHCI controller, starts it and enumerates
//!   every port, hubs included.
//! - [`poll`] (called by the keyboard driver whenever input is read) keeps
//!   keyboard transfers running, decodes reports into the keyboard input
//!   buffer, and handles root ports connecting or disconnecting (e.g. a
//!   KVM switch).
//!
//! The controller is polled; no USB interrupts are used. Single CPU: the
//! kernel is not preempted, and the USB state is only touched from boot
//! and from `poll` (never from interrupt handlers).

pub mod device;
pub mod hid;
pub mod pci;
pub mod ring;
pub mod xhci;

/// USB driver errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbError {
    NoController,
    InitFailed,
    NoMemory,
    Timeout,
    CommandFailed(u8),
    TransferFailed(u8),
    NotSupported,
}

pub type Result<T> = core::result::Result<T, UsbError>;

/// Print to the screen and the debug port
pub(crate) fn log(s: &str) {
    crate::drivers::display::write_str(s);
    for &b in s.as_bytes() {
        unsafe { core::arch::asm!("out dx, al", in("dx") 0xE9u16, in("al") b, options(nomem, nostack)) };
    }
}

pub(crate) fn log_hex(v: u64, digits: usize) {
    let h = b"0123456789ABCDEF";
    let mut buf = [0u8; 16];
    let digits = digits.min(16);
    for i in 0..digits {
        buf[i] = h[((v >> (4 * (digits - 1 - i))) & 0xF) as usize];
    }
    log(unsafe { core::str::from_utf8_unchecked(&buf[..digits]) });
}

pub(crate) fn log_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    log(unsafe { core::str::from_utf8_unchecked(&buf[i..]) });
}

/// Find and start the USB controller and enumerate its devices.
/// Returns the number of keyboards found. Safe to call once at boot after
/// the PMM, heap and TSC calibration.
pub fn init() -> usize {
    log("[USB] Scanning PCI for xHCI controllers...\n");
    let found = match pci::find_xhci() {
        Some(f) => f,
        None => {
            log("[USB] No xHCI controller\n");
            return 0;
        }
    };
    unsafe {
        match xhci::Controller::new(found.mmio_phys) {
            Ok(c) => xhci::set_controller(c),
            Err(e) => {
                log("[USB] Controller init failed: ");
                log(error_name(e));
                log("\n");
                return 0;
            }
        }
        let n = device::enumerate_all();
        log("[USB] ");
        log_dec(n as u64);
        log(if n == 1 { " keyboard ready\n" } else { " keyboards ready\n" });
        n
    }
}

/// Service the USB keyboards and port changes (call often while waiting
/// for input). Decoded keys go to the keyboard input buffer.
pub fn poll() {
    unsafe { device::poll() }
}

/// True if a controller is running
pub fn active() -> bool {
    unsafe { xhci::controller().map_or(false, |c| !c.is_dead()) }
}

pub(crate) fn error_name(e: UsbError) -> &'static str {
    match e {
        UsbError::NoController => "no controller",
        UsbError::InitFailed => "init failed",
        UsbError::NoMemory => "out of memory",
        UsbError::Timeout => "timeout",
        UsbError::CommandFailed(_) => "command failed",
        UsbError::TransferFailed(_) => "transfer failed",
        UsbError::NotSupported => "not supported",
    }
}
