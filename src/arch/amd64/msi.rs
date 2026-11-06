// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Handlers for the MSI vectors (0x40-0x7F). Each records the interrupt
//! for its interrupt object (if one is bound) and signals end of interrupt
//! to the local APIC. MSIs are edge-triggered: nothing to mask.

use super::idt::idt_set_gate;
use crate::kobj::interrupt::{Interrupt, MSI_COUNT, MSI_FIRST};

/// What the CPU pushes on an interrupt from either privilege level
#[repr(C)]
pub struct Frame {
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

fn eoi() {
    unsafe {
        let lapic = super::mm::kernel_map::phys_to_virt(0xFEE0_0000) as usize;
        core::ptr::write_volatile((lapic + 0xB0) as *mut u32, 0);
    }
}

macro_rules! handlers {
    ($($n:literal)*) => {
        const HANDLERS: [extern "x86-interrupt" fn(Frame); MSI_COUNT as usize] = [$({
            extern "x86-interrupt" fn h(_f: Frame) {
                Interrupt::dispatch_vector(MSI_FIRST + $n);
                eoi();
            }
            h
        }),*];
    };
}

handlers!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31
          32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63);

/// Put the handlers in the IDT (boot CPU, before interrupts are enabled;
/// the IDT is shared by all CPUs)
pub fn install() {
    for (i, h) in HANDLERS.iter().enumerate() {
        unsafe { idt_set_gate((MSI_FIRST as usize + i) as u8, *h as u64, 0x08, 0x8E) };
    }
}
