// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! PCI configuration access (mechanism #1, ports 0xCF8/0xCFC) and the
//! search for an xHCI controller.

use super::{log, log_hex};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn address(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    0x8000_0000 | (bus as u32) << 16 | (dev as u32) << 11 | (func as u32) << 8 | (reg as u32 & 0xFC)
}

/// Read a 32-bit configuration register
pub fn read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") CONFIG_ADDRESS, in("eax") address(bus, dev, func, reg), options(nomem, nostack));
        core::arch::asm!("in eax, dx", in("dx") CONFIG_DATA, out("eax") v, options(nomem, nostack));
    }
    v
}

/// Write a 32-bit configuration register
pub fn write32(bus: u8, dev: u8, func: u8, reg: u8, value: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") CONFIG_ADDRESS, in("eax") address(bus, dev, func, reg), options(nomem, nostack));
        core::arch::asm!("out dx, eax", in("dx") CONFIG_DATA, in("eax") value, options(nomem, nostack));
    }
}

/// An xHCI controller found on the bus
#[derive(Clone, Copy)]
pub struct XhciFunction {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub mmio_phys: u64,
}

/// Memory BAR 0 (32 or 64 bit)
fn bar0(bus: u8, dev: u8, func: u8) -> Option<u64> {
    let lo = read32(bus, dev, func, 0x10);
    if lo & 1 != 0 {
        return None; // I/O BAR
    }
    let base = match (lo >> 1) & 3 {
        0 => (lo & 0xFFFF_FFF0) as u64,
        2 => ((read32(bus, dev, func, 0x14) as u64) << 32) | (lo & 0xFFFF_FFF0) as u64,
        _ => return None,
    };
    if base == 0 { None } else { Some(base) }
}

/// Scan every bus/device/function for xHCI controllers (class 0C/03/30),
/// print them, and return the preferred one with memory decoding and bus
/// mastering enabled. The chipset controller (bus 0) is preferred: it owns
/// the USB-A ports and the built-in devices; Thunderbolt and GPU USB-C
/// controllers sit on other buses.
pub fn find_xhci() -> Option<XhciFunction> {
    let mut best: Option<XhciFunction> = None;
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            let id = read32(bus, dev, 0, 0);
            if id & 0xFFFF == 0xFFFF {
                continue;
            }
            let multi = (read32(bus, dev, 0, 0x0C) >> 16) & 0x80 != 0;
            for func in 0..if multi { 8 } else { 1 } {
                let id = read32(bus, dev, func, 0);
                if id & 0xFFFF == 0xFFFF {
                    continue;
                }
                let class = read32(bus, dev, func, 0x08) >> 8; // class:subclass:prog-if
                if class != 0x0C0330 {
                    continue;
                }
                let mmio = match bar0(bus, dev, func) {
                    Some(m) => m,
                    None => continue,
                };
                let f = XhciFunction { bus, dev, func, vendor: id as u16, device: (id >> 16) as u16, mmio_phys: mmio };
                log("[USB]   ");
                log_hex(bus as u64, 2);
                log(":");
                log_hex(dev as u64, 2);
                log(".");
                log_hex(func as u64, 1);
                log(" ");
                log_hex(f.vendor as u64, 4);
                log(":");
                log_hex(f.device as u64, 4);
                log(" xHCI at 0x");
                log_hex(mmio, 16);
                log("\n");
                if best.map_or(true, |b| b.bus != 0 && bus == 0) {
                    best = Some(f);
                }
            }
        }
        if bus == 255 {
            break;
        }
    }
    let f = best?;
    // Memory space + bus master; status bits written as 0 (RW1C: no effect)
    let cmd = read32(f.bus, f.dev, f.func, 0x04) & 0xFFFF;
    write32(f.bus, f.dev, f.func, 0x04, cmd | 0x6);
    log("[USB] Using ");
    log_hex(f.bus as u64, 2);
    log(":");
    log_hex(f.dev as u64, 2);
    log(".");
    log_hex(f.func as u64, 1);
    log("\n");
    Some(f)
}
