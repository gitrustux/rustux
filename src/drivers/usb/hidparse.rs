// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Pointing devices (DESKTOP.md step F3): the little of a HID report
//! descriptor a mouse or tablet needs, where its buttons, X, Y and wheel
//! are in its input reports, and those reports turned into pointer
//! events for the input channel.
//!
//! A pointer event is 8 bytes, two of the channel's 4-byte records:
//! `[buttons, wheel (i8), flags, 0x81]` then X and Y as little-endian
//! 16-bit numbers. Flags bit 0: absolute (X and Y 0..=32767 across the
//! screen, as from QEMU's usb-tablet); otherwise X and Y are signed
//! relative motion. Key events have 0 in their fourth byte.

/// Where one field is in a report (bits from the start, after the report ID)
#[derive(Clone, Copy, Default)]
pub struct Field {
    pub bit: u16,
    pub size: u8,
    pub signed: bool,
    pub present: bool,
    pub min: i32,
    pub max: i32,
}

#[derive(Clone, Copy, Default)]
pub struct Layout {
    /// 0: reports carry no ID byte
    pub report_id: u8,
    pub buttons: Field,
    pub nbuttons: u8,
    pub x: Field,
    pub y: Field,
    pub wheel: Field,
    pub absolute: bool,
}

/// The layout of a boot-protocol mouse report: buttons, X, Y (i8), wheel
pub fn boot_mouse() -> Layout {
    let f = |bit, size, signed| Field { bit, size, signed, present: true, min: -127, max: 127 };
    Layout {
        report_id: 0,
        buttons: Field { bit: 0, size: 1, signed: false, present: true, min: 0, max: 1 },
        nbuttons: 3,
        x: f(8, 8, true),
        y: f(16, 8, true),
        wheel: Field::default(),
        absolute: false,
    }
}

/// Read a report descriptor; Some if it has X and Y in an input report
/// (the first report ID that does)
pub fn parse(desc: &[u8]) -> Option<Layout> {
    let mut l = Layout::default();
    let (mut page, mut size, mut count, mut lmin, mut lmax, mut rid) = (0u32, 0u32, 0u32, 0i32, 0i32, 0u8);
    let mut usages = [0u32; 16];
    let mut nusages = 0usize;
    let (mut umin, mut umax) = (0u32, 0u32);
    // Bits so far in the input report of each ID (ID 0..15)
    let mut offset = [0u32; 16];
    let mut chosen: Option<u8> = None;
    let mut i = 0;
    while i < desc.len() {
        let b = desc[i];
        if b == 0xFE {
            // Long item: skip
            if i + 1 >= desc.len() {
                break;
            }
            i += 3 + desc[i + 1] as usize;
            continue;
        }
        let n = match b & 3 {
            3 => 4,
            s => s as usize,
        };
        if i + 1 + n > desc.len() {
            break;
        }
        let mut u = 0u32;
        for k in 0..n {
            u |= (desc[i + 1 + k] as u32) << (8 * k);
        }
        let s = match n {
            1 => u as u8 as i8 as i32,
            2 => u as u16 as i16 as i32,
            _ => u as i32,
        };
        match b & 0xFC {
            // Global items
            0x04 => page = u,
            0x14 => lmin = s,
            0x24 => lmax = if lmin >= 0 && n < 4 { u as i32 } else { s },
            0x74 => size = u,
            0x94 => count = u,
            0x84 => rid = u as u8,
            // Local items
            0x08 => {
                if nusages < usages.len() {
                    usages[nusages] = if n == 4 { u } else { page << 16 | u };
                    nusages += 1;
                }
            }
            0x18 => umin = if n == 4 { u } else { page << 16 | u },
            0x28 => umax = if n == 4 { u } else { page << 16 | u },
            // Main items
            0x80 => {
                // Input
                let id = (rid & 15) as usize;
                let constant = u & 1 != 0;
                let relative = u & 4 != 0;
                let variable = u & 2 != 0;
                for k in 0..count {
                    let usage = if nusages > 0 {
                        usages[core::cmp::min(k as usize, nusages - 1)]
                    } else if umax >= umin && umin != 0 {
                        core::cmp::min(umin + k, umax)
                    } else {
                        0
                    };
                    let bit = offset[id] + k * size;
                    let field = Field {
                        bit: bit as u16,
                        size: size as u8,
                        signed: lmin < 0,
                        present: true,
                        min: lmin,
                        max: lmax,
                    };
                    if constant || !variable || chosen.map_or(false, |c| c != rid) || size == 0 || size > 16 {
                        continue;
                    }
                    match usage {
                        0x0001_0030 => {
                            l.x = field;
                            l.absolute = !relative;
                            chosen = Some(rid);
                        }
                        0x0001_0031 => {
                            l.y = field;
                            chosen = Some(rid);
                        }
                        0x0001_0038 => l.wheel = field,
                        0x0009_0001..=0x0009_0008 => {
                            if !l.buttons.present {
                                l.buttons = field;
                            }
                            l.nbuttons = core::cmp::max(l.nbuttons, (usage - 0x0009_0000) as u8);
                        }
                        _ => {}
                    }
                }
                offset[id] += count * size;
                nusages = 0;
                umin = 0;
                umax = 0;
            }
            0x90 | 0xB0 | 0xA0 | 0xC0 => {
                // Output, feature, collection, end collection
                if b & 0xFC == 0x90 || b & 0xFC == 0xB0 {
                    // Not part of input reports
                }
                nusages = 0;
                umin = 0;
                umax = 0;
            }
            _ => {}
        }
        i += 1 + n;
    }
    let id = chosen?;
    if !l.x.present || !l.y.present {
        return None;
    }
    l.report_id = id;
    Some(l)
}

fn get(report: &[u8], f: &Field) -> i32 {
    if !f.present {
        return 0;
    }
    let mut v: u32 = 0;
    for k in 0..f.size as usize {
        let bit = f.bit as usize + k;
        if bit / 8 < report.len() && report[bit / 8] & (1 << (bit % 8)) != 0 {
            v |= 1 << k;
        }
    }
    if f.signed && f.size < 32 && v & (1 << (f.size - 1)) != 0 {
        (v | !((1u32 << f.size) - 1)) as i32
    } else {
        v as i32
    }
}

/// One input report to a pointer event (pushed to the input channel)
pub fn process_report(report: &[u8], l: &Layout) {
    let r = if l.report_id != 0 {
        match report.split_first() {
            Some((&id, rest)) if id == l.report_id => rest,
            _ => return,
        }
    } else {
        report
    };
    let mut buttons = 0u8;
    if l.buttons.present {
        for b in 0..core::cmp::min(l.nbuttons, 8) {
            let f = Field { bit: l.buttons.bit + b as u16, size: 1, signed: false, ..l.buttons };
            if get(r, &f) != 0 {
                buttons |= 1 << b;
            }
        }
    }
    let wheel = get(r, &l.wheel).clamp(-127, 127) as i8;
    let (x, y) = if l.absolute {
        let scale = |f: &Field| -> u16 {
            let v = get(r, f);
            let span = (f.max - f.min).max(1) as i64;
            (((v - f.min).clamp(0, span as i32) as i64 * 32767) / span) as u16
        };
        (scale(&l.x), scale(&l.y))
    } else {
        (get(r, &l.x).clamp(-32768, 32767) as i16 as u16, get(r, &l.y).clamp(-32768, 32767) as i16 as u16)
    };
    for b in [buttons, wheel as u8, l.absolute as u8, 0x81, x as u8, (x >> 8) as u8, y as u8, (y >> 8) as u8] {
        crate::drivers::keyboard::push_input(b);
    }
}
