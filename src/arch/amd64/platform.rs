// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Platform services: the CMOS real-time clock (wall-clock time at boot),
//! and restarting or powering off the machine through ACPI.
//!
//! Power-off follows the usual minimal ACPI path: find the `_S5_` package
//! in the DSDT for the sleep type values, then write them with SLP_EN to
//! the PM1 control registers named in the FADT. Restart tries the FADT
//! reset register, then the 8042 keyboard controller, then port 0xCF9, and
//! finally a triple fault.

use super::mm::kernel_map;
use super::smp::find_table;
use core::sync::atomic::{AtomicI64, Ordering};

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn outb(port: u16, v: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack, preserves_flags));
}
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn outw(port: u16, v: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") v, options(nomem, nostack, preserves_flags));
}
unsafe fn rd8(p: u64) -> u8 {
    *(kernel_map::phys_to_virt(p) as *const u8)
}
unsafe fn rd32(p: u64) -> u32 {
    core::ptr::read_unaligned(kernel_map::phys_to_virt(p) as *const u32)
}
unsafe fn rd64(p: u64) -> u64 {
    core::ptr::read_unaligned(kernel_map::phys_to_virt(p) as *const u64)
}

// ---------------------------------------------------------------------
// Real-time clock
// ---------------------------------------------------------------------

/// UTC (ns since 1970) minus the monotonic clock: wall time = monotonic +
/// this. Set at boot from the RTC; init may correct it (`clock_set_utc`).
static UTC_OFFSET: AtomicI64 = AtomicI64::new(0);

unsafe fn cmos(reg: u8) -> u8 {
    outb(0x70, reg);
    inb(0x71)
}

/// Days from 1970-01-01 to `y-m-d` (proleptic Gregorian)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Read the RTC (assumed to hold UTC) as seconds since 1970
pub fn rtc_read() -> i64 {
    unsafe {
        let century_reg = find_table(b"FACP").map(|f| rd8(f + 108)).unwrap_or(0);
        let read = || {
            // Wait for an update cycle to finish
            let mut n = 0;
            while cmos(0x0A) & 0x80 != 0 && n < 1_000_000 {
                n += 1;
            }
            [cmos(0x00), cmos(0x02), cmos(0x04), cmos(0x07), cmos(0x08), cmos(0x09),
             if century_reg != 0 { cmos(century_reg) } else { 0 }]
        };
        // Read until two reads agree (an update may happen in between)
        let mut a = read();
        loop {
            let b = read();
            if a == b {
                break;
            }
            a = b;
        }
        let status_b = cmos(0x0B);
        let bcd = |v: u8| if status_b & 0x04 == 0 { (v & 0x0F) + (v >> 4) * 10 } else { v };
        let sec = bcd(a[0]) as i64;
        let min = bcd(a[1]) as i64;
        let mut hour = bcd(a[2] & 0x7F) as i64;
        if status_b & 0x02 == 0 && a[2] & 0x80 != 0 {
            hour = (hour % 12) + 12; // 12-hour mode, PM
        } else if status_b & 0x02 == 0 && hour == 12 {
            hour = 0;
        }
        let day = bcd(a[3]) as i64;
        let month = bcd(a[4]) as i64;
        let mut year = bcd(a[5]) as i64;
        year += if century_reg != 0 && a[6] != 0 { bcd(a[6]) as i64 * 100 } else { 2000 };
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return 0;
        }
        days_from_civil(year, month, day) * 86400 + hour * 3600 + min * 60 + sec
    }
}

/// Monotonic time when the kernel set up its clock (for uptime)
static BOOT_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn boot_ns() -> u64 {
    BOOT_NS.load(Ordering::Acquire)
}

/// Set the wall clock from the RTC (at boot)
pub fn init_clock() {
    BOOT_NS.store(crate::kobj::now_ns(), Ordering::Release);
    let utc_ns = rtc_read() as i128 * 1_000_000_000;
    let mono = crate::kobj::now_ns() as i128;
    UTC_OFFSET.store((utc_ns - mono) as i64, Ordering::Release);
}

/// Wall-clock time, ns since 1970 (UTC)
pub fn utc_now() -> i64 {
    crate::kobj::now_ns() as i64 + UTC_OFFSET.load(Ordering::Acquire)
}

/// Set the wall-clock time
pub fn set_utc(utc_ns: i64) {
    UTC_OFFSET.store(utc_ns - crate::kobj::now_ns() as i64, Ordering::Release);
}

// ---------------------------------------------------------------------
// Power
// ---------------------------------------------------------------------

/// The `_S5_` sleep type values from the DSDT, if found
unsafe fn s5_types(fadt: u64) -> Option<(u16, u16)> {
    let len = rd32(fadt + 4);
    let dsdt = if len >= 148 && rd64(fadt + 140) != 0 { rd64(fadt + 140) } else { rd32(fadt + 40) as u64 };
    if dsdt == 0 {
        return None;
    }
    let dlen = rd32(dsdt + 4) as u64;
    let body = core::slice::from_raw_parts(kernel_map::phys_to_virt(dsdt) as *const u8, dlen as usize);
    let at = body.windows(4).position(|w| w == b"_S5_")?;
    // NameOp '_S5_' PackageOp PkgLength NumElements elements...
    let mut i = at + 4;
    if body.get(i)? != &0x12 {
        return None;
    }
    i += 1;
    i += ((body.get(i)? >> 6) & 3) as usize + 1; // PkgLength: lead byte + follow bytes
    i += 1; // NumElements
    let mut element = || -> Option<u16> {
        let v = match *body.get(i)? {
            0x0A => { let v = *body.get(i + 1)? as u16; i += 2; v } // BytePrefix
            0x0B => { let v = u16::from_le_bytes([*body.get(i + 1)?, *body.get(i + 2)?]); i += 3; v } // WordPrefix
            0x00 => { i += 1; 0 } // ZeroOp
            0x01 => { i += 1; 1 } // OneOp
            b => { i += 1; b as u16 }
        };
        Some(v)
    };
    let a = element()?;
    let b = element().unwrap_or(0);
    Some((a, b))
}

/// The I/O port in a FADT field: the 32-bit block address, or the X_
/// generic address if that one is an I/O port
unsafe fn fadt_port(fadt: u64, legacy: u64, extended: u64) -> u16 {
    let len = rd32(fadt + 4) as u64;
    if len >= extended + 12 && rd8(fadt + extended) == 1 && rd64(fadt + extended + 4) != 0 {
        return rd64(fadt + extended + 4) as u16;
    }
    rd32(fadt + legacy) as u16
}

/// Turn the machine off (ACPI S5). Returns only if that did not work.
pub fn power_off() {
    unsafe {
        let Some(fadt) = find_table(b"FACP") else { return };
        let Some((typ_a, typ_b)) = s5_types(fadt) else { return };
        let pm1a = fadt_port(fadt, 64, 172);
        let pm1b = fadt_port(fadt, 68, 184);
        // Switch to ACPI mode if the firmware left it off (SCI_EN clear)
        let smi = rd32(fadt + 48) as u16;
        let enable = rd8(fadt + 52);
        if pm1a != 0 && inw(pm1a) & 1 == 0 && smi != 0 && enable != 0 {
            outb(smi, enable);
            for _ in 0..3_000_000 {
                if inw(pm1a) & 1 != 0 {
                    break;
                }
            }
        }
        const SLP_EN: u16 = 1 << 13;
        if pm1a != 0 {
            outw(pm1a, (inw(pm1a) & 0xE3FF) | (typ_a << 10) | SLP_EN);
        }
        if pm1b != 0 {
            outw(pm1b, (inw(pm1b) & 0xE3FF) | (typ_b << 10) | SLP_EN);
        }
        // The machine needs a moment to cut power
        for _ in 0..50_000_000 {
            core::hint::spin_loop();
        }
    }
}

/// Restart the machine. Never returns.
pub fn reboot() -> ! {
    unsafe {
        // 1. ACPI reset register (FADT flag RESET_REG_SUP, bit 10)
        if let Some(fadt) = find_table(b"FACP") {
            let len = rd32(fadt + 4);
            if len >= 129 && rd32(fadt + 112) & (1 << 10) != 0 {
                let space = rd8(fadt + 116);
                let addr = rd64(fadt + 120);
                let value = rd8(fadt + 128);
                match space {
                    1 => outb(addr as u16, value),
                    0 => core::ptr::write_volatile(kernel_map::phys_to_virt(addr) as *mut u8, value),
                    _ => {}
                }
                for _ in 0..10_000_000 {
                    core::hint::spin_loop();
                }
            }
        }
        // 2. Keyboard controller: pulse the reset line
        for _ in 0..100_000 {
            if inb(0x64) & 2 == 0 {
                break;
            }
        }
        outb(0x64, 0xFE);
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }
        // 3. PCI reset control register: full reset
        outb(0xCF9, 0x02);
        outb(0xCF9, 0x06);
        for _ in 0..10_000_000 {
            core::hint::spin_loop();
        }
        // 4. Triple fault: an empty IDT and an exception
        let idt: [u16; 5] = [0; 5];
        core::arch::asm!("lidt [{}]", "int3", in(reg) idt.as_ptr(), options(nostack));
    }
    loop {
        unsafe { core::arch::asm!("cli", "hlt", options(nomem, nostack)) };
    }
}
