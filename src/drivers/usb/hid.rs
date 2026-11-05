// Copyright 2026 The Rustux Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! HID boot-protocol keyboard reports to key events.
//!
//! Since step 10 the kernel no longer turns keys into characters: it sends
//! key events to `init`, which applies the keyboard layout (the same for
//! USB and PS/2). An event is 4 bytes: HID usage (page 0x07; modifiers are
//! 0xE0-0xE7), the modifier byte after the event (HID bit order), flags
//! (bit 0 pressed; bit 1 the device repeats keys itself, which USB boot
//! keyboards don't), and 0.

/// Turn one report into press and release events. `prev` holds the same
/// keyboard's previous report (modifier byte, then 6 keys) and is updated.
pub fn process_report(report: &[u8], prev: &mut [u8; 7]) {
    if report.len() < 3 {
        return;
    }
    let modifier = report[0];
    let mut keys = [0u8; 6];
    for (i, k) in report[2..].iter().take(6).enumerate() {
        keys[i] = *k;
    }
    if keys[0] == 1 {
        return; // rollover error: no information, keep the previous state
    }
    let old_mod = prev[0];
    let old_keys: [u8; 6] = prev[1..7].try_into().unwrap();
    // Modifier changes, as key events of usages 0xE0-0xE7
    for bit in 0..8u8 {
        let (was, is) = (old_mod & (1 << bit) != 0, modifier & (1 << bit) != 0);
        if was != is {
            push(0xE0 + bit, modifier, is);
        }
    }
    for &k in old_keys.iter() {
        if k > 3 && !keys.contains(&k) {
            push(k, modifier, false);
        }
    }
    for &k in keys.iter() {
        if k > 3 && !old_keys.contains(&k) {
            push(k, modifier, true);
        }
    }
    prev[0] = modifier;
    prev[1..7].copy_from_slice(&keys);
}

fn push(usage: u8, modifier: u8, pressed: bool) {
    for b in [usage, modifier, pressed as u8, 0] {
        crate::drivers::keyboard::push_input(b);
    }
}
